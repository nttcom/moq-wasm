use anyhow::{Result as AnyResult, anyhow};
use broadcast::{
    Broadcast, BroadcastWriter, RelayConnection, RenditionConfig, RenditionKeyframes,
    RenditionTrack, SourceTracks,
};
use bytes::Bytes;
use js_sys::Promise;
use mediapack::{
    AudioConfig, AudioLevel, AudioSample, MediaEvent, Timestamp,
    aac::AudioSpecificConfig,
    loc::{Demuxer as LocDemuxer, LocExtension, LocObject, TIMESTAMP_ID, VideoCodec},
};
use moqt::executor::{self, JoinHandle};
use tokio::sync::{mpsc, oneshot};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

use crate::js_error;

/// An MSF broadcast of media the page encodes: it publishes the catalog, the
/// LOC tracks and the media timeline, keeps objects for FETCH and answers
/// SUBSCRIBE. Created with `Session.broadcast`. The tracks of every source
/// share its one catalog.
#[wasm_bindgen]
pub struct MediaBroadcast {
    broadcast: Broadcast,
}

#[wasm_bindgen]
pub struct MediaSource {
    writer: BroadcastWriter,
    tracks: SourceTracks,
    /// Reads a chunk as it reads a received LOC object: into the track's
    /// configuration changes and a sample.
    video: Option<LocDemuxer>,
    renditions: Vec<Rendition>,
}

/// One rendition of the video, written by a task of its own in the order its
/// events and removals arrive.
struct Rendition {
    command_sender: mpsc::UnboundedSender<RenditionCommand>,
    video: Option<LocDemuxer>,
    _task: JoinHandle,
}

enum RenditionCommand {
    Event(MediaEvent),
    Remove(oneshot::Sender<AnyResult<()>>),
}

impl MediaBroadcast {
    pub(crate) fn new(connection: RelayConnection, namespace: Vec<String>) -> Self {
        Self {
            broadcast: Broadcast::new(connection, namespace),
        }
    }
}

#[wasm_bindgen]
impl MediaBroadcast {
    /// Resolves once the namespace is announced (PUBLISH_NAMESPACE).
    pub async fn announce(&self) -> Result<(), JsValue> {
        self.broadcast
            .announce()
            .await
            .map_err(|err| js_error(format!("{err:#}")))
    }

    /// Only a source on the tracks `video` and `audio` brings the media
    /// timeline and the chat. Its video forms a switching set of its own with
    /// its renditions.
    #[wasm_bindgen(js_name = addSource)]
    pub fn add_source(
        &self,
        video: String,
        video_label: String,
        audio: String,
        audio_label: String,
    ) -> MediaSource {
        let tracks = SourceTracks {
            switching_set: video.clone(),
            video,
            video_label,
            audio,
            audio_label,
            cmaf: false,
        };
        MediaSource {
            writer: BroadcastWriter::run(self.broadcast.clone(), tracks.clone()),
            tracks,
            video: None,
            renditions: Vec::new(),
        }
    }
}

impl MediaSource {
    fn rendition(&mut self, index: usize) -> Result<&mut Rendition, JsValue> {
        self.renditions
            .get_mut(index)
            .ok_or_else(|| js_error(format!("no rendition {index}")))
    }

    fn push(&mut self, event: MediaEvent) -> Result<(), JsValue> {
        self.writer
            .push(event)
            .map_err(|err| js_error(format!("{err:#}")))
    }
}

#[wasm_bindgen]
impl MediaSource {
    /// Ends the source's video or audio track (`kind` is `video` or `audio`)
    /// with PUBLISH_DONE and sends the catalog without it; stop pushing its
    /// media first. `configureVideo` or `configureAudio` and the media pushed
    /// after it publish the track again.
    #[wasm_bindgen(js_name = removeTrack)]
    pub fn remove_track(&self, kind: String) -> Result<Promise, JsValue> {
        let track_name = match kind.as_str() {
            "video" => &self.tracks.video,
            "audio" => &self.tracks.audio,
            other => return Err(js_error(format!("unknown track kind: {other}"))),
        };
        // Not an async method: wasm-bindgen would hold the borrow of `self`
        // until it resolved, failing every push of the other kind meanwhile.
        let removal = self.writer.remove_track(track_name);
        let renditions: Vec<_> = if kind == "video" {
            self.renditions
                .iter()
                .map(|rendition| {
                    let (done_sender, done_receiver) = oneshot::channel();
                    let sent = rendition
                        .command_sender
                        .send(RenditionCommand::Remove(done_sender));
                    async move {
                        sent.map_err(|_| anyhow!("rendition task ended"))?;
                        done_receiver
                            .await
                            .map_err(|_| anyhow!("rendition task ended"))?
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        Ok(future_to_promise(async move {
            let mut result = removal.await;
            for removed in renditions {
                result = result.and(removed.await);
            }
            result
                .map(|()| JsValue::UNDEFINED)
                .map_err(|err| js_error(format!("{err:#}")))
        }))
    }

    /// Adds a rendition of the video in the track `<video track>_<name>`, in the
    /// group ids of the video's keyframes on the same frames
    /// (draft-ietf-moq-cmsf-01 §3.2), and returns its index. `bitrate` is in
    /// bits per second. Removing the video removes its renditions.
    #[wasm_bindgen(js_name = addRendition)]
    pub fn add_rendition(&mut self, name: String, label: String, bitrate: Option<f64>) -> usize {
        let config = RenditionConfig {
            track: format!("{}_{name}", self.tracks.video),
            label,
            bitrate: bitrate.map(|bitrate| bitrate as u64),
            cmaf: false,
            keyframes: RenditionKeyframes::SameFrames,
        };
        let broadcast = self.writer.broadcast().clone();
        let track_namespace = broadcast.namespace().join("/");
        let track_name = config.track.clone();
        let mut track = RenditionTrack::new(&self.writer, config);
        let (command_sender, mut command_receiver) = mpsc::unbounded_channel();
        let task = executor::spawn("media-broadcast-rendition", async move {
            while let Some(command) = command_receiver.recv().await {
                match command {
                    RenditionCommand::Event(event) => {
                        if let Err(error) = track.push(event).await {
                            tracing::warn!(%error, %track_namespace, %track_name, "rendition push failed");
                        }
                    }
                    RenditionCommand::Remove(done_sender) => {
                        let _ = done_sender.send(broadcast.remove_track(&track_name).await);
                    }
                }
            }
        });
        self.renditions.push(Rendition {
            command_sender,
            video: None,
            _task: task,
        });
        self.renditions.len() - 1
    }

    /// `configureVideo` for the rendition at `index`.
    #[wasm_bindgen(js_name = configureRendition)]
    pub fn configure_rendition(&mut self, index: usize, codec: String) -> Result<(), JsValue> {
        let demuxer = video_demuxer(&codec)?;
        self.rendition(index)?.video = Some(demuxer);
        Ok(())
    }

    /// `pushVideo` for the rendition at `index`: the chunk of the same frame
    /// as a video chunk, with the same timestamp.
    #[wasm_bindgen(js_name = pushRenditionVideo)]
    pub fn push_rendition_video(
        &mut self,
        index: usize,
        data: Vec<u8>,
        timestamp_micros: f64,
    ) -> Result<(), JsValue> {
        let rendition = self.rendition(index)?;
        let video = rendition
            .video
            .as_mut()
            .ok_or_else(|| js_error("call configureRendition first"))?;
        let events = video
            .push(&chunk(data, timestamp_micros))
            .map_err(|err| js_error(format!("{err:#}")))?;
        for event in events {
            rendition
                .command_sender
                .send(RenditionCommand::Event(event))
                .map_err(|_| js_error("rendition task ended"))?;
        }
        Ok(())
    }

    /// `codec` is the WebCodecs codec string: `avc1.*` or `avc3.*` with
    /// `avc: { format: 'annexb' }`, `vp8`, `vp09.*` or `av01.*`; fails for any
    /// other.
    #[wasm_bindgen(js_name = configureVideo)]
    pub fn configure_video(&mut self, codec: String) -> Result<(), JsValue> {
        self.video = Some(video_demuxer(&codec)?);
        Ok(())
    }

    /// Call `configureVideo` first. A keyframe sets the track's configuration
    /// (for H.264, its SPS and PPS), published again when it changes.
    /// `timestamp_micros` must not run backwards.
    #[wasm_bindgen(js_name = pushVideo)]
    pub fn push_video(&mut self, data: Vec<u8>, timestamp_micros: f64) -> Result<(), JsValue> {
        let video = self
            .video
            .as_mut()
            .ok_or_else(|| js_error("call configureVideo first"))?;
        let chunk = chunk(data, timestamp_micros);
        let events = video
            .push(&chunk)
            .map_err(|err| js_error(format!("{err:#}")))?;
        for event in events {
            self.push(event)?;
        }
        Ok(())
    }

    /// `codec` is the WebCodecs codec string: `opus`, or an AAC `mp4a.40.*`
    /// whose `description` is the AudioSpecificConfig.
    #[wasm_bindgen(js_name = configureAudio)]
    pub fn configure_audio(
        &mut self,
        codec: String,
        sample_rate: u32,
        channels: u8,
        description: Option<Vec<u8>>,
    ) -> Result<(), JsValue> {
        let config = match codec.as_str() {
            "opus" => AudioConfig::Opus {
                sample_rate,
                channels,
            },
            aac if aac.starts_with("mp4a.40.") => {
                let description =
                    description.ok_or_else(|| js_error("AAC needs its AudioSpecificConfig"))?;
                AudioConfig::Aac(
                    AudioSpecificConfig::parse(&description)
                        .map_err(|err| js_error(format!("{err:#}")))?,
                )
            }
            other => return Err(js_error(format!("unsupported audio codec: {other}"))),
        };
        self.push(MediaEvent::AudioConfig(config))
    }

    /// One encoded audio frame; call `configureAudio` first. `level` is the
    /// frame's RFC 6464 level in -dBov (0 to 127), written with
    /// `voice_activity` as its LOC Audio Level property.
    #[wasm_bindgen(js_name = pushAudio)]
    pub fn push_audio(
        &mut self,
        data: Vec<u8>,
        timestamp_micros: f64,
        level: Option<u8>,
        voice_activity: Option<bool>,
    ) -> Result<(), JsValue> {
        self.push(MediaEvent::Audio(AudioSample {
            data: Bytes::from(data),
            pts: Timestamp::from_micros(timestamp_micros as u64),
            audio_level: level.map(|level| AudioLevel {
                level,
                voice_activity: voice_activity.unwrap_or(false),
            }),
        }))
    }
}

fn video_demuxer(codec: &str) -> Result<LocDemuxer, JsValue> {
    let video_codec = VideoCodec::from_codec_string(codec)
        .ok_or_else(|| js_error(format!("unsupported video codec: {codec}")))?;
    Ok(LocDemuxer::video(video_codec).with_capture_origin(Timestamp::ZERO))
}

fn chunk(data: Vec<u8>, timestamp_micros: f64) -> LocObject {
    LocObject {
        extensions: vec![LocExtension::varint(TIMESTAMP_ID, timestamp_micros as u64)],
        payload: Bytes::from(data),
    }
}
