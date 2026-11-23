use std::sync::{Arc, Mutex};

use broadcast::{
    Broadcast, DemandChanges, MediaRendition, MediaSourceConfig, MediaSourceError, PushOutcome,
    TrackKind,
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

const CLOSED: &str = "the media broadcast was closed";

/// A source of the broadcast; `None` once `MediaBroadcast.close` ended it.
type SharedSource = Arc<Mutex<Option<broadcast::MediaSource>>>;

/// An MSF broadcast of media the caller encodes, built by `broadcast` as the
/// browser SDK's is: it publishes the catalog, the LOC tracks and the media
/// timeline, keeps objects for FETCH and answers SUBSCRIBE. Its tracks share
/// the session's origin, so the session's own tracks in the namespace are
/// still answered.
#[pyclass(module = "moq_wasm")]
pub struct MediaBroadcast {
    broadcast: Broadcast,
    /// The sources `close` ends; `None` once closed.
    sources: Mutex<Option<Vec<SharedSource>>>,
    session: Arc<client::Session>,
    handler: HandlerId,
    scope: SessionScope,
}

impl MediaBroadcast {
    pub(crate) fn new(
        broadcast: Broadcast,
        session: Arc<client::Session>,
        handler: HandlerId,
        scope: SessionScope,
    ) -> Self {
        Self {
            broadcast,
            sources: Mutex::new(Some(Vec::new())),
            session,
            handler,
            scope,
        }
    }
}

#[pymethods]
impl MediaBroadcast {
    fn announce<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        if self.sources.lock().unwrap().is_none() {
            return Err(PyRuntimeError::new_err(CLOSED));
        }
        let broadcast = self.broadcast.clone();
        self.scope.future_into_py(py, async move {
            broadcast
                .announce()
                .await
                .map_err(|error| request_error_in(error, RequestKind::PublishNamespace))?;
            Ok(())
        })
    }

    /// Stops answering for the broadcast's tracks, ends them and the catalog
    /// with PUBLISH_DONE after the media pushed before, and stops the
    /// catalog refresh; later frames raise. Withdraw the namespace with
    /// `Session.unannounce` first. A failed step, as once the session ended,
    /// does not stop the others. Does nothing the second time.
    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.session.remove_handler(self.handler);
        let sources = self.sources.lock().unwrap().take();
        let ends: Vec<_> = sources
            .iter()
            .flatten()
            .filter_map(|source| source.lock().unwrap().take())
            .map(|source| self.scope.run(|| source.end()))
            .collect();
        let broadcast = self.broadcast.clone();
        self.scope.future_into_py(py, async move {
            if sources.is_none() {
                return Ok(());
            }
            for end in ends {
                report_teardown(end.await);
            }
            report_teardown(broadcast.end_catalog().await);
            Ok(())
        })
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
    ) -> PyResult<MediaSource> {
        let mut sources = self.sources.lock().unwrap();
        let sources = sources
            .as_mut()
            .ok_or_else(|| PyRuntimeError::new_err(CLOSED))?;
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
        let keyframe_requests = source.keyframe_requests();
        let demands = source.demand_changes();
        let source = Arc::new(Mutex::new(Some(source)));
        sources.push(source.clone());
        Ok(MediaSource {
            source,
            scope: self.scope,
            keyframe_requests,
            demands,
        })
    }
}

fn report_teardown(result: anyhow::Result<()>) {
    if let Err(error) = result {
        tracing::debug!(%error, "media broadcast teardown step failed");
    }
}

/// One source of a `MediaBroadcast`: a video and an audio track, fed with
/// encoded frames.
#[pyclass(module = "moq_wasm")]
pub struct MediaSource {
    source: SharedSource,
    scope: SessionScope,
    keyframe_requests: Arc<Notify>,
    /// `None` for a source not published on demand.
    demands: Option<DemandChanges>,
}

impl MediaSource {
    fn with<R>(
        &self,
        call: impl FnOnce(&mut broadcast::MediaSource) -> PyResult<R>,
    ) -> PyResult<R> {
        self.scope.run(|| {
            let mut source = self.source.lock().unwrap();
            call(
                source
                    .as_mut()
                    .ok_or_else(|| PyRuntimeError::new_err(CLOSED))?,
            )
        })
    }
}

#[pymethods]
impl MediaSource {
    /// `codec` is a WebCodecs codec string: `avc1.*`, `hvc1.*`, `vp8`,
    /// `vp09.*` or `av01.*`; H.264 and H.265 frames are Annex B.
    fn configure_video(&self, codec: &str) -> PyResult<()> {
        self.with(|source| source.configure_video(codec).map_err(py_error))
    }

    /// One encoded frame; call `configure_video` first. A keyframe opens a
    /// group and sets the track's configuration (for H.264 and H.265, its
    /// parameter sets). `timestamp_micros` must not run backwards. Returns
    /// whether the frame was queued: false for one the publish queue
    /// dropped while the relay does not keep up, and for one without a
    /// picture, as an H.264 access unit without a slice, which is not
    /// published; the parameter sets of one without a slice still go to the
    /// keyframes after it.
    fn push_video(&self, data: Vec<u8>, timestamp_micros: u64) -> PyResult<bool> {
        self.with(|source| queued(source.push_video(Bytes::from(data), timestamp_micros)))
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
    ) -> PyResult<Rendition> {
        let _runtime = pyo3_async_runtimes::tokio::get_runtime().enter();
        let rendition =
            self.with(|source| Ok(source.add_rendition(name, label, bitrate, framerate)))?;
        Ok(Rendition {
            rendition: Mutex::new(rendition),
            source: self.source.clone(),
            scope: self.scope,
        })
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
        let removal = self.with(|source| Ok(source.remove_track(kind)))?;
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
        self.with(|source| {
            source
                .configure_audio(codec, sample_rate, channels, description.as_deref())
                .map_err(py_error)
        })
    }

    /// `level` is the frame's RFC 6464 level in -dBov (0 to 127), written
    /// with `voice_activity` as its LOC Audio Level property. Returns whether
    /// the frame was queued: false for one the publish queue dropped while
    /// the relay does not keep up.
    #[pyo3(signature = (data, timestamp_micros, *, level = None, voice_activity = false))]
    fn push_audio(
        &self,
        data: Vec<u8>,
        timestamp_micros: u64,
        level: Option<u8>,
        voice_activity: bool,
    ) -> PyResult<bool> {
        let level = level.map(|level| AudioLevel {
            level,
            voice_activity,
        });
        self.with(|source| queued(source.push_audio(Bytes::from(data), timestamp_micros, level)))
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
    /// The slot of its source, which `MediaBroadcast.close` empties.
    source: SharedSource,
    scope: SessionScope,
}

impl Rendition {
    fn with<R>(&self, call: impl FnOnce(&mut MediaRendition) -> PyResult<R>) -> PyResult<R> {
        if self.source.lock().unwrap().is_none() {
            return Err(PyRuntimeError::new_err(CLOSED));
        }
        self.scope.run(|| call(&mut self.rendition.lock().unwrap()))
    }
}

#[pymethods]
impl Rendition {
    /// As `MediaSource.configure_video`.
    fn configure_video(&self, codec: &str) -> PyResult<()> {
        self.with(|rendition| rendition.configure_video(codec).map_err(py_error))
    }

    /// An encoded frame of the same picture as a video frame, at that
    /// frame's timestamp. Returns false when the rendition's queue, full
    /// while the relay does not keep up, dropped the frame, and for a frame
    /// without a picture, as `MediaSource.push_video`. Fails once an earlier
    /// frame stopped the rendition.
    fn push_video(&self, data: Vec<u8>, timestamp_micros: u64) -> PyResult<bool> {
        self.with(|rendition| queued(rendition.push_video(Bytes::from(data), timestamp_micros)))
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

/// Whether a push was queued: false for a frame dropped or without a picture.
fn queued(outcome: Result<PushOutcome, MediaSourceError>) -> PyResult<bool> {
    Ok(matches!(outcome.map_err(py_error)?, PushOutcome::Queued))
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
