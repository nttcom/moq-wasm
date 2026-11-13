use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use broadcast::{Broadcast, BroadcastWriter, RelayConnection, SourceTracks};
use bytes::Bytes;
use mediapack::{
    AudioConfig, AudioSample, MediaEvent, Timestamp,
    loc::{Demuxer as LocDemuxer, LocExtension, LocObject, TIMESTAMP_ID, VideoCodec},
};
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
};
use tokio::sync::Notify;

/// The connections of a session's media broadcasts, which see each event
/// before the session's origin.
#[derive(Clone, Default)]
pub(crate) struct MediaBroadcasts {
    connections: Arc<Mutex<Vec<(u64, RelayConnection)>>>,
    next_id: Arc<AtomicU64>,
}

impl MediaBroadcasts {
    pub(crate) fn add(&self, connection: RelayConnection) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.connections.lock().unwrap().push((id, connection));
        id
    }

    pub(crate) fn connections(&self) -> Vec<RelayConnection> {
        let connections = self.connections.lock().unwrap();
        connections
            .iter()
            .map(|(_, connection)| connection.clone())
            .collect()
    }

    fn remove(&self, id: u64) {
        self.connections
            .lock()
            .unwrap()
            .retain(|(entry, _)| *entry != id);
    }
}

/// An MSF broadcast of media the caller encodes, built by `broadcast` as the
/// browser SDK's is: it publishes the catalog, the LOC tracks and the media
/// timeline, keeps objects for FETCH and answers SUBSCRIBE. Its tracks share
/// the session's origin, so the session's own tracks in the namespace are
/// still answered.
#[pyclass(module = "moq_wasm")]
pub struct MediaBroadcast {
    pub(crate) broadcast: Broadcast,
    pub(crate) broadcasts: MediaBroadcasts,
    pub(crate) id: u64,
}

#[pymethods]
impl MediaBroadcast {
    fn announce<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let broadcast = self.broadcast.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            broadcast.announce().await?;
            Ok(())
        })
    }

    /// Stops answering for the broadcast's tracks; withdraw the namespace
    /// with `Session.unannounce` first.
    fn close(&self) {
        self.broadcasts.remove(self.id);
    }

    /// A source on the tracks `video` and `audio`, with the media timeline
    /// `<video>_timeline`. `video_bitrate` is the encoder's target in bits per
    /// second and `video_framerate` the frames per second it is capped at, if
    /// it is; both go into the catalog.
    #[pyo3(signature = (video, audio, *, label, video_bitrate = None, video_framerate = None))]
    fn add_source(
        &self,
        video: String,
        audio: String,
        label: String,
        video_bitrate: Option<u64>,
        video_framerate: Option<u32>,
    ) -> MediaSource {
        let keyframe_requests = Arc::new(Notify::new());
        let tracks = SourceTracks {
            switching_set: video.clone(),
            video,
            audio_label: format!("{label} audio"),
            video_label: label,
            audio,
            cmaf: false,
            video_bitrate,
            video_framerate,
            keyframe_requests: Some(keyframe_requests.clone()),
            demand_sender: None,
        };
        // The writer spawns its publish task, which needs the runtime the module's futures run on.
        let _runtime = pyo3_async_runtimes::tokio::get_runtime().enter();
        MediaSource {
            writer: Arc::new(Mutex::new(BroadcastWriter::run(
                self.broadcast.clone(),
                tracks,
            ))),
            video: Mutex::new(None),
            keyframe_requests,
        }
    }
}

/// One source of a `MediaBroadcast`: a video and an audio track, fed with
/// encoded frames.
#[pyclass(module = "moq_wasm")]
pub struct MediaSource {
    writer: Arc<Mutex<BroadcastWriter>>,
    /// Reads a frame as it reads a received LOC object: into the track's
    /// configuration changes and a sample.
    video: Mutex<Option<LocDemuxer>>,
    keyframe_requests: Arc<Notify>,
}

impl MediaSource {
    fn push(&self, event: MediaEvent) -> PyResult<()> {
        self.writer
            .lock()
            .unwrap()
            .push(event)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }
}

#[pymethods]
impl MediaSource {
    /// `codec` is a WebCodecs codec string: `avc1.*`, `hvc1.*`, `vp8`,
    /// `vp09.*` or `av01.*`; H.264 and H.265 frames are Annex B.
    fn configure_video(&self, codec: &str) -> PyResult<()> {
        let video_codec = VideoCodec::from_codec_string(codec)
            .ok_or_else(|| PyValueError::new_err(format!("unsupported video codec: {codec}")))?;
        *self.video.lock().unwrap() =
            Some(LocDemuxer::video(video_codec).with_capture_origin(Timestamp::ZERO));
        Ok(())
    }

    /// One encoded frame; call `configure_video` first. A keyframe opens a
    /// group and sets the track's configuration (for H.264 and H.265, its
    /// parameter sets). `timestamp_micros` must not run backwards.
    fn push_video(&self, data: Vec<u8>, timestamp_micros: u64) -> PyResult<()> {
        let events = {
            let mut video = self.video.lock().unwrap();
            let video = video
                .as_mut()
                .ok_or_else(|| PyRuntimeError::new_err("call configure_video first"))?;
            video
                .push(&LocObject {
                    extensions: vec![LocExtension::varint(TIMESTAMP_ID, timestamp_micros)],
                    payload: Bytes::from(data),
                })
                .map_err(|error| PyValueError::new_err(error.to_string()))?
        };
        for event in events {
            self.push(event)?;
        }
        Ok(())
    }

    fn configure_audio(&self, codec: &str, sample_rate: u32, channels: u8) -> PyResult<()> {
        if codec != "opus" {
            return Err(PyValueError::new_err(format!(
                "unsupported audio codec: {codec}"
            )));
        }
        self.push(MediaEvent::AudioConfig(AudioConfig::Opus {
            sample_rate,
            channels,
        }))
    }

    fn push_audio(&self, data: Vec<u8>, timestamp_micros: u64) -> PyResult<()> {
        self.push(MediaEvent::Audio(AudioSample {
            data: Bytes::from(data),
            pts: Timestamp::from_micros(timestamp_micros),
            audio_level: None,
        }))
    }

    /// Resolves the next time a subscriber asks for a new group
    /// (NEW_GROUP_REQUEST) or the broadcast needs one, as after dropping
    /// frames; encode the next frame as a keyframe then.
    fn keyframe_request<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let requests = self.keyframe_requests.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            requests.notified().await;
            Ok(())
        })
    }
}
