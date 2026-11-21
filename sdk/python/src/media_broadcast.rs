use std::sync::{Arc, Mutex};

use broadcast::{
    Broadcast, DemandChanges, MediaRendition, MediaSourceConfig, MediaSourceError, TrackKind,
};
use bytes::Bytes;
use mediapack::{AudioLevel, aac::AudioSpecificConfig};
use moqt::{
    client::{self, HandlerId},
    wire::RequestKind,
};
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
};
use tokio::sync::Notify;

use crate::{errors::request_error_in, scope::SessionScope};

/// An MSF broadcast of media the caller encodes, built by `broadcast` as the
/// browser SDK's is: it publishes the catalog, the LOC tracks and the media
/// timeline, keeps objects for FETCH and answers SUBSCRIBE. Its tracks share
/// the session's origin, so the session's own tracks in the namespace are
/// still answered.
#[pyclass(module = "moq_wasm")]
pub struct MediaBroadcast {
    pub(crate) broadcast: Broadcast,
    pub(crate) session: Arc<client::Session>,
    pub(crate) handler: HandlerId,
    pub(crate) scope: SessionScope,
}

#[pymethods]
impl MediaBroadcast {
    fn announce<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let broadcast = self.broadcast.clone();
        self.scope.future_into_py(py, async move {
            broadcast
                .announce()
                .await
                .map_err(|error| request_error_in(error, RequestKind::PublishNamespace))?;
            Ok(())
        })
    }

    /// Stops answering for the broadcast's tracks; withdraw the namespace
    /// with `Session.unannounce` first.
    fn close(&self) {
        self.session.remove_handler(self.handler);
    }

    /// A source on the tracks `video` and `audio`, with the media timeline
    /// `<video>_timeline`. `video_bitrate` is the encoder's target in bits per
    /// second and `video_framerate` the frames per second it is capped at, if
    /// it is; both go into the catalog.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (
        video, audio, *, label, video_bitrate = None, video_framerate = None, cmaf = false,
        on_demand = true
    ))]
    fn add_source(
        &self,
        video: String,
        audio: String,
        label: String,
        video_bitrate: Option<u64>,
        video_framerate: Option<u32>,
        cmaf: bool,
        on_demand: bool,
    ) -> MediaSource {
        let config = MediaSourceConfig {
            video,
            audio,
            label,
            video_bitrate,
            video_framerate,
            cmaf,
            on_demand,
        };
        // The source spawns its publish task, which needs the runtime the module's futures run on
        // and keeps the scope it is spawned in.
        let _runtime = pyo3_async_runtimes::tokio::get_runtime().enter();
        let source = self
            .scope
            .run(|| broadcast::MediaSource::new(self.broadcast.clone(), config));
        MediaSource {
            keyframe_requests: source.keyframe_requests(),
            demands: source.demand_changes(),
            source: Mutex::new(source),
            scope: self.scope,
        }
    }
}

/// One source of a `MediaBroadcast`: a video and an audio track, fed with
/// encoded frames.
#[pyclass(module = "moq_wasm")]
pub struct MediaSource {
    source: Mutex<broadcast::MediaSource>,
    scope: SessionScope,
    keyframe_requests: Arc<Notify>,
    /// `None` for a source not published on demand.
    demands: Option<DemandChanges>,
}

#[pymethods]
impl MediaSource {
    /// `codec` is a WebCodecs codec string: `avc1.*`, `hvc1.*`, `vp8`,
    /// `vp09.*` or `av01.*`; H.264 and H.265 frames are Annex B.
    fn configure_video(&self, codec: &str) -> PyResult<()> {
        self.source
            .lock()
            .unwrap()
            .configure_video(codec)
            .map_err(py_error)
    }

    /// One encoded frame; call `configure_video` first. A keyframe opens a
    /// group and sets the track's configuration (for H.264 and H.265, its
    /// parameter sets). `timestamp_micros` must not run backwards.
    fn push_video(&self, data: Vec<u8>, timestamp_micros: u64) -> PyResult<()> {
        self.scope.run(|| {
            self.source
                .lock()
                .unwrap()
                .push_video(Bytes::from(data), timestamp_micros)
                .map(drop)
                .map_err(py_error)
        })
    }

    /// A rendition of the video in the track `<video>_<name>`, in the group
    /// ids of the video's keyframes on the same frames
    /// (draft-ietf-moq-cmsf-01 §3.2); it joins the video's switching set, so
    /// one of another codec is an alternative to it.
    #[pyo3(signature = (name, *, label, bitrate = None, framerate = None))]
    fn add_rendition(
        &self,
        name: &str,
        label: String,
        bitrate: Option<u64>,
        framerate: Option<u32>,
    ) -> Rendition {
        let _runtime = pyo3_async_runtimes::tokio::get_runtime().enter();
        let rendition = self.scope.run(|| {
            self.source
                .lock()
                .unwrap()
                .add_rendition(name, label, bitrate, framerate)
        });
        Rendition {
            rendition: Mutex::new(rendition),
            scope: self.scope,
        }
    }

    /// Ends the source's video or audio track (`kind` is `video` or `audio`)
    /// with PUBLISH_DONE and sends the catalog without it; removing the video
    /// removes its renditions too. Stop pushing its media first;
    /// `configure_video` or `configure_audio` and the media pushed after it
    /// publish the track again. Fails when the relay session has ended.
    fn remove_track<'py>(&self, py: Python<'py>, kind: &str) -> PyResult<Bound<'py, PyAny>> {
        let kind = match kind {
            "video" => TrackKind::Video,
            "audio" => TrackKind::Audio,
            other => {
                return Err(PyValueError::new_err(format!(
                    "unknown track kind: {other}"
                )));
            }
        };
        let removal = self.source.lock().unwrap().remove_track(kind);
        self.scope
            .future_into_py(py, async move { Ok(removal.await?) })
    }

    /// `codec` is `opus`, or an AAC `mp4a.40.*` whose `description` is the
    /// AudioSpecificConfig.
    #[pyo3(signature = (codec, sample_rate, channels, *, description = None))]
    fn configure_audio(
        &self,
        codec: &str,
        sample_rate: u32,
        channels: u8,
        description: Option<Vec<u8>>,
    ) -> PyResult<()> {
        self.scope.run(|| {
            self.source
                .lock()
                .unwrap()
                .configure_audio(codec, sample_rate, channels, description.as_deref())
                .map_err(py_error)
        })
    }

    /// `level` is the frame's RFC 6464 level in -dBov (0 to 127), written
    /// with `voice_activity` as its LOC Audio Level property.
    #[pyo3(signature = (data, timestamp_micros, *, level = None, voice_activity = false))]
    fn push_audio(
        &self,
        data: Vec<u8>,
        timestamp_micros: u64,
        level: Option<u8>,
        voice_activity: bool,
    ) -> PyResult<()> {
        let level = level.map(|level| AudioLevel {
            level,
            voice_activity,
        });
        self.scope.run(|| {
            self.source
                .lock()
                .unwrap()
                .push_audio(Bytes::from(data), timestamp_micros, level)
                .map(drop)
                .map_err(py_error)
        })
    }

    /// Resolves the next time a subscriber asks for a new group
    /// (NEW_GROUP_REQUEST) or the broadcast needs one, as after dropping
    /// frames; encode the next frame as a keyframe then.
    fn keyframe_request<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let requests = self.keyframe_requests.clone();
        self.scope.future_into_py(py, async move {
            requests.notified().await;
            Ok(())
        })
    }

    /// The next change of whether a subscriber takes one of the source's
    /// tracks, the video's and renditions' CMAF siblings included, for a
    /// source published on demand. A change replaces its track's unread one,
    /// so a late reader gets each track's latest. Fails once every track of
    /// the source is gone.
    fn demand_change<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let next = self
            .demands
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("the source was added with on_demand=False"))?
            .next();
        self.scope.future_into_py(py, async move {
            let demand = next
                .await
                .ok_or_else(|| PyRuntimeError::new_err("the media source stopped publishing"))?;
            Ok(Demand {
                track_name: demand.track_name,
                wanted: demand.wanted,
            })
        })
    }
}

/// One rendition of a `MediaSource`'s video, fed with encoded frames of the
/// same pictures as the video.
#[pyclass(module = "moq_wasm")]
pub struct Rendition {
    rendition: Mutex<MediaRendition>,
    scope: SessionScope,
}

#[pymethods]
impl Rendition {
    /// As `MediaSource.configure_video`.
    fn configure_video(&self, codec: &str) -> PyResult<()> {
        self.rendition
            .lock()
            .unwrap()
            .configure_video(codec)
            .map_err(py_error)
    }

    /// An encoded frame of the same picture as a video frame, at that
    /// frame's timestamp. Fails once an earlier frame stopped the rendition.
    fn push_video(&self, data: Vec<u8>, timestamp_micros: u64) -> PyResult<()> {
        self.scope.run(|| {
            self.rendition
                .lock()
                .unwrap()
                .push_video(Bytes::from(data), timestamp_micros)
                .map(drop)
                .map_err(py_error)
        })
    }
}

/// Raises ValueError for bytes that are no AudioSpecificConfig
/// (ISO/IEC 14496-3 §1.6.2.1).
#[pyfunction]
pub fn check_audio_specific_config(description: &[u8]) -> PyResult<()> {
    AudioSpecificConfig::parse(description)
        .map(drop)
        .map_err(|error| PyValueError::new_err(error.to_string()))
}

fn py_error(error: MediaSourceError) -> PyErr {
    match error {
        MediaSourceError::InvalidMedia(_) => PyValueError::new_err(error.to_string()),
        _ => PyRuntimeError::new_err(error.to_string()),
    }
}

/// Whether a subscriber takes `track_name` (draft-18 §10.10).
#[pyclass(frozen, get_all, module = "moq_wasm")]
pub struct Demand {
    pub track_name: String,
    pub wanted: bool,
}
