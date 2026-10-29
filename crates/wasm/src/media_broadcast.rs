use broadcast::{Broadcast, BroadcastWriter, RelayConnection, SourceTracks};
use bytes::Bytes;
use js_sys::Promise;
use mediapack::{
    AudioConfig, AudioLevel, AudioSample, MediaEvent, Timestamp,
    aac::AudioSpecificConfig,
    loc::{Demuxer as LocDemuxer, LocExtension, LocObject, TIMESTAMP_ID, VideoCodec},
};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

use crate::js_error;

/// An MSF broadcast of media the page encodes: it publishes the catalog, the
/// LOC tracks and the media timeline, keeps objects for FETCH and answers
/// SUBSCRIBE. Created with `Session.broadcast`.
#[wasm_bindgen]
pub struct MediaBroadcast {
    writer: BroadcastWriter,
    tracks: SourceTracks,
    /// Reads a chunk as it reads a received LOC object: into the track's
    /// configuration changes and a sample.
    video: Option<LocDemuxer>,
}

impl MediaBroadcast {
    pub(crate) fn new(
        connection: RelayConnection,
        namespace: Vec<String>,
        video_label: Option<String>,
    ) -> Self {
        let mut tracks = SourceTracks {
            cmaf: false,
            ..SourceTracks::default()
        };
        if let Some(video_label) = video_label {
            tracks.video_label = video_label;
        }
        Self {
            writer: BroadcastWriter::run(Broadcast::new(connection, namespace), tracks.clone()),
            tracks,
            video: None,
        }
    }

    fn push(&mut self, event: MediaEvent) -> Result<(), JsValue> {
        self.writer
            .push(event)
            .map_err(|err| js_error(format!("{err:#}")))
    }
}

#[wasm_bindgen]
impl MediaBroadcast {
    /// Resolves once the namespace is announced (PUBLISH_NAMESPACE).
    pub async fn announce(&self) -> Result<(), JsValue> {
        self.writer
            .broadcast()
            .announce()
            .await
            .map_err(|err| js_error(format!("{err:#}")))
    }

    /// Ends the `video` or `audio` track with PUBLISH_DONE and sends the catalog
    /// without it; stop pushing its media first. `configureVideo` or
    /// `configureAudio` and the media pushed after it publish the track again.
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
        Ok(future_to_promise(async move {
            removal
                .await
                .map(|()| JsValue::UNDEFINED)
                .map_err(|err| js_error(format!("{err:#}")))
        }))
    }

    /// `codec` is the WebCodecs codec string: `avc1.*` or `avc3.*` with
    /// `avc: { format: 'annexb' }`, `vp8`, `vp09.*` or `av01.*`; fails for any
    /// other.
    #[wasm_bindgen(js_name = configureVideo)]
    pub fn configure_video(&mut self, codec: String) -> Result<(), JsValue> {
        let video_codec = VideoCodec::from_codec_string(&codec)
            .ok_or_else(|| js_error(format!("unsupported video codec: {codec}")))?;
        self.video = Some(LocDemuxer::video(video_codec).with_capture_origin(Timestamp::ZERO));
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
        let chunk = LocObject {
            extensions: vec![LocExtension::varint(TIMESTAMP_ID, timestamp_micros as u64)],
            payload: Bytes::from(data),
        };
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
