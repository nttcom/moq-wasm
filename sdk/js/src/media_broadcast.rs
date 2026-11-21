mod demand_task;
mod keyframe_request_task;

use broadcast::{
    Broadcast, MediaRendition, MediaSourceConfig, MediaSourceError, RelayConnection, TrackKind,
};
use bytes::Bytes;
use js_sys::{Function, Promise};
use mediapack::AudioLevel;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

use crate::{anyhow_error, js_error, log_sink::SessionScope};
use demand_task::DemandTask;
use keyframe_request_task::KeyframeRequestTask;

/// An MSF broadcast of media the page encodes: it publishes the catalog, the
/// LOC tracks and the media timeline, keeps objects for FETCH and answers
/// SUBSCRIBE. Created with `Session.broadcast`. The tracks of every source
/// share its one catalog.
#[wasm_bindgen]
pub struct MediaBroadcast {
    broadcast: Broadcast,
    scope: SessionScope,
}

#[wasm_bindgen]
pub struct MediaSource {
    source: broadcast::MediaSource,
    scope: SessionScope,
    renditions: Vec<MediaRendition>,
    _keyframe_requests: KeyframeRequestTask,
    _demand: Option<DemandTask>,
}

impl MediaBroadcast {
    pub(crate) fn new(
        connection: RelayConnection,
        namespace: Vec<String>,
        scope: SessionScope,
    ) -> Self {
        Self {
            broadcast: Broadcast::new(connection, namespace),
            scope,
        }
    }
}

#[wasm_bindgen]
impl MediaBroadcast {
    /// Resolves once the namespace is announced (PUBLISH_NAMESPACE).
    pub async fn announce(&self) -> Result<(), JsValue> {
        self.scope
            .scoped(self.broadcast.announce())
            .await
            .map_err(anyhow_error)
    }

    /// Announces the namespace like `announce` and accepts SUBSCRIBEs for
    /// `track_names` before the catalog lists them.
    #[wasm_bindgen(js_name = declareTracks)]
    pub async fn declare_tracks(&self, track_names: Vec<String>) -> Result<(), JsValue> {
        self.scope
            .scoped(self.broadcast.declare_tracks(&track_names))
            .await
            .map_err(anyhow_error)
    }

    /// Lists the chat track in the catalog even while no source publishes the
    /// default `video` or `audio`; write the chat with `Session.publishTrack`.
    #[wasm_bindgen(js_name = offerChat)]
    pub fn offer_chat(&self) -> Promise {
        let broadcast = self.broadcast.clone();
        future_to_promise(self.scope.scoped(async move {
            broadcast
                .offer_chat()
                .await
                .map(|()| JsValue::UNDEFINED)
                .map_err(anyhow_error)
        }))
    }

    /// Lists `name` in the catalog as a JSON event timeline
    /// (draft-ietf-moq-msf-02 §8) whose records' `data` is of `event_type`.
    /// Write it with `Session.publishTrack` given the same `priority`, 128
    /// when absent, since that call sets the track's priority again. Offering
    /// a name again replaces its entry. Fails once the relay session has ended.
    #[wasm_bindgen(js_name = offerEventTimeline)]
    pub fn offer_event_timeline(
        &self,
        name: String,
        event_type: String,
        priority: Option<u8>,
    ) -> Promise {
        let broadcast = self.broadcast.clone();
        let mut track = msf::Track::new(&name, msf::KnownPackaging::EventTimeline, true);
        track.namespace = Some(broadcast.namespace().join("/"));
        track.role = Some(msf::TrackRole::Other(name));
        track.event_type = Some(event_type);
        track.mime_type = Some("application/json".to_string());
        future_to_promise(self.scope.scoped(async move {
            broadcast
                .offer_event_timeline(
                    track,
                    priority.unwrap_or(moqt::client::PublishTrackOptions::default().priority),
                )
                .await
                .map(|()| JsValue::UNDEFINED)
                .map_err(anyhow_error)
        }))
    }

    /// Only a source on the tracks `video` and `audio` brings the chat. Every
    /// source brings its media timeline, `<video>_timeline`. Its video forms
    /// a switching set of its own with its renditions. The audio is labelled
    /// `<video_label> audio`.
    /// `video_bitrate` is the video encoder's target in bits per second, and
    /// `video_framerate` the frames per second it is capped at, if it is.
    /// The video and its renditions have dynamic groups (draft-18 §12.6):
    /// `on_keyframe_request` is called, without arguments, when a subscriber
    /// asks for a new group, and the page then encodes the next frame of the
    /// video and of every rendition as a keyframe.
    /// The video, its renditions and the audio are published on demand
    /// (draft-18 §10.10), so the relay has each one sent only while a
    /// subscriber takes it: `on_demand_change` is called with a track's name
    /// and whether one does, when the track is published and at every change,
    /// and the page need not encode a track nobody takes.
    /// With `cmaf`, the video and its renditions also travel as CMAF in
    /// `<track>_cmaf` (draft-ietf-moq-cmsf-01), sent only while a subscriber
    /// takes it and reported to `on_demand_change` under that name.
    #[wasm_bindgen(js_name = addSource)]
    #[allow(clippy::too_many_arguments)]
    pub fn add_source(
        &self,
        video: String,
        video_label: String,
        video_bitrate: Option<f64>,
        video_framerate: Option<f64>,
        audio: String,
        cmaf: bool,
        on_keyframe_request: Function,
        on_demand_change: Function,
    ) -> MediaSource {
        let config = MediaSourceConfig {
            video,
            audio,
            label: video_label,
            video_bitrate: video_bitrate.map(|bitrate| bitrate as u64),
            video_framerate: video_framerate.map(|framerate| framerate as u32),
            cmaf,
            on_demand: true,
        };
        let mut source = self
            .scope
            .in_scope(|| broadcast::MediaSource::new(self.broadcast.clone(), config));
        let demand = source.take_demand_receiver().map(|demand_receiver| {
            self.scope
                .in_scope(|| DemandTask::run(demand_receiver, on_demand_change))
        });
        let keyframe_requests = source.keyframe_requests();
        let keyframe_request_task = self
            .scope
            .in_scope(|| KeyframeRequestTask::run(keyframe_requests, on_keyframe_request));
        MediaSource {
            source,
            scope: self.scope,
            renditions: Vec::new(),
            _keyframe_requests: keyframe_request_task,
            _demand: demand,
        }
    }
}

impl MediaSource {
    fn rendition(&mut self, index: usize) -> Result<&mut MediaRendition, JsValue> {
        self.renditions
            .get_mut(index)
            .ok_or_else(|| js_error(format!("no rendition {index}")))
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
        let kind = match kind.as_str() {
            "video" => TrackKind::Video,
            "audio" => TrackKind::Audio,
            other => return Err(js_error(format!("unknown track kind: {other}"))),
        };
        // Not an async method: wasm-bindgen would hold the borrow of `self`
        // until it resolved, failing every push of the other kind meanwhile.
        let removal = self.source.remove_track(kind);
        Ok(future_to_promise(self.scope.scoped(async move {
            removal
                .await
                .map(|()| JsValue::UNDEFINED)
                .map_err(anyhow_error)
        })))
    }

    /// Adds a rendition of the video in the track `<video track>_<name>`, in the
    /// group ids of the video's keyframes on the same frames
    /// (draft-ietf-moq-cmsf-01 §3.2), and returns its index. `bitrate` is in
    /// bits per second, and `framerate` the frames per second the rendition is
    /// capped at, if it is. Removing the video removes its renditions.
    #[wasm_bindgen(js_name = addRendition)]
    pub fn add_rendition(
        &mut self,
        name: String,
        label: String,
        bitrate: Option<f64>,
        framerate: Option<f64>,
    ) -> usize {
        let rendition = self.scope.in_scope(|| {
            self.source.add_rendition(
                &name,
                label,
                bitrate.map(|bitrate| bitrate as u64),
                framerate.map(|framerate| framerate as u32),
            )
        });
        self.renditions.push(rendition);
        self.renditions.len() - 1
    }

    /// `configureVideo` for the rendition at `index`.
    #[wasm_bindgen(js_name = configureRendition)]
    pub fn configure_rendition(&mut self, index: usize, codec: String) -> Result<(), JsValue> {
        self.rendition(index)?
            .configure_video(&codec)
            .map_err(source_error)
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
        self.rendition(index)?
            .push_video(Bytes::from(data), timestamp_micros as u64)
            .map(drop)
            .map_err(source_error)
    }

    /// `codec` is the WebCodecs codec string: `avc1.*` or `avc3.*` with
    /// `avc: { format: 'annexb' }`, `hvc1.*` or `hev1.*` with
    /// `hevc: { format: 'annexb' }`, `vp8`, `vp09.*` or `av01.*`; fails for
    /// any other.
    #[wasm_bindgen(js_name = configureVideo)]
    pub fn configure_video(&mut self, codec: String) -> Result<(), JsValue> {
        self.source.configure_video(&codec).map_err(source_error)
    }

    /// Call `configureVideo` first. A keyframe sets the track's configuration
    /// (for H.264 and H.265, its parameter sets), published again when it
    /// changes.
    /// `timestamp_micros` must not run backwards.
    #[wasm_bindgen(js_name = pushVideo)]
    pub fn push_video(&mut self, data: Vec<u8>, timestamp_micros: f64) -> Result<(), JsValue> {
        self.source
            .push_video(Bytes::from(data), timestamp_micros as u64)
            .map(drop)
            .map_err(source_error)
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
        self.source
            .configure_audio(&codec, sample_rate, channels, description.as_deref())
            .map_err(source_error)
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
        let level = level.map(|level| AudioLevel {
            level,
            voice_activity: voice_activity.unwrap_or(false),
        });
        self.source
            .push_audio(Bytes::from(data), timestamp_micros as u64, level)
            .map(drop)
            .map_err(source_error)
    }
}

fn source_error(error: MediaSourceError) -> JsValue {
    js_error(error.to_string())
}
