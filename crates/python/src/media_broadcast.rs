mod rendition_task;

use std::sync::{Arc, Mutex};

use broadcast::{
    Broadcast, BroadcastWriter, RenditionConfig, RenditionKeyframes, RenditionTrack, SourceTracks,
};
use bytes::Bytes;
use mediapack::{
    AudioConfig, AudioLevel, AudioSample, MediaEvent, Timestamp,
    aac::AudioSpecificConfig,
    loc::{Demuxer as LocDemuxer, LocExtension, LocObject, TIMESTAMP_ID, VideoCodec},
};
use moqt::client::{self, HandlerId};
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
};
use tokio::sync::Notify;

use rendition_task::RenditionTask;

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
        self.session.remove_handler(self.handler);
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
        *self.video.lock().unwrap() = Some(video_demuxer(codec)?);
        Ok(())
    }

    /// One encoded frame; call `configure_video` first. A keyframe opens a
    /// group and sets the track's configuration (for H.264 and H.265, its
    /// parameter sets). `timestamp_micros` must not run backwards.
    fn push_video(&self, data: Vec<u8>, timestamp_micros: u64) -> PyResult<()> {
        for event in demux(&self.video, data, timestamp_micros)? {
            self.push(event)?;
        }
        Ok(())
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
        let writer = self.writer.lock().unwrap();
        let source_tracks = writer.source_tracks();
        let track = format!("{}_{name}", source_tracks.video);
        let config = RenditionConfig {
            track: track.clone(),
            label,
            bitrate,
            framerate,
            cmaf: source_tracks.cmaf,
            keyframes: RenditionKeyframes::SameFrames,
        };
        let rendition = RenditionTrack::new(&writer, config);
        Rendition {
            task: RenditionTask::run(rendition, track),
            video: Mutex::new(None),
        }
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
        let config = match codec {
            "opus" => AudioConfig::Opus {
                sample_rate,
                channels,
            },
            aac if aac.starts_with("mp4a.40.") => {
                let description = description
                    .ok_or_else(|| PyValueError::new_err("AAC needs its AudioSpecificConfig"))?;
                AudioConfig::Aac(audio_specific_config(&description)?)
            }
            other => {
                return Err(PyValueError::new_err(format!(
                    "unsupported audio codec: {other}"
                )));
            }
        };
        self.push(MediaEvent::AudioConfig(config))
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
        self.push(MediaEvent::Audio(AudioSample {
            data: Bytes::from(data),
            pts: Timestamp::from_micros(timestamp_micros),
            audio_level: level.map(|level| AudioLevel {
                level,
                voice_activity,
            }),
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

/// One rendition of a `MediaSource`'s video, fed with encoded frames of the
/// same pictures as the video.
#[pyclass(module = "moq_wasm")]
pub struct Rendition {
    task: RenditionTask,
    video: Mutex<Option<LocDemuxer>>,
}

#[pymethods]
impl Rendition {
    /// As `MediaSource.configure_video`.
    fn configure_video(&self, codec: &str) -> PyResult<()> {
        *self.video.lock().unwrap() = Some(video_demuxer(codec)?);
        Ok(())
    }

    /// An encoded frame of the same picture as a video frame, at that
    /// frame's timestamp. Fails once an earlier frame stopped the rendition.
    fn push_video(&self, data: Vec<u8>, timestamp_micros: u64) -> PyResult<()> {
        for event in demux(&self.video, data, timestamp_micros)? {
            self.task
                .push(event)
                .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        }
        Ok(())
    }
}

/// Raises ValueError for bytes that are no AudioSpecificConfig
/// (ISO/IEC 14496-3 §1.6.2.1).
#[pyfunction]
pub fn check_audio_specific_config(description: &[u8]) -> PyResult<()> {
    audio_specific_config(description).map(drop)
}

fn audio_specific_config(description: &[u8]) -> PyResult<AudioSpecificConfig> {
    AudioSpecificConfig::parse(description)
        .map_err(|error| PyValueError::new_err(error.to_string()))
}

fn video_demuxer(codec: &str) -> PyResult<LocDemuxer> {
    let video_codec = VideoCodec::from_codec_string(codec)
        .ok_or_else(|| PyValueError::new_err(format!("unsupported video codec: {codec}")))?;
    Ok(LocDemuxer::video(video_codec).with_capture_origin(Timestamp::ZERO))
}

/// Reads a frame as it reads a received LOC object: into the track's
/// configuration changes and a sample.
fn demux(
    video: &Mutex<Option<LocDemuxer>>,
    data: Vec<u8>,
    timestamp_micros: u64,
) -> PyResult<Vec<MediaEvent>> {
    let mut video = video.lock().unwrap();
    let video = video
        .as_mut()
        .ok_or_else(|| PyRuntimeError::new_err("call configure_video first"))?;
    video
        .push(&LocObject {
            extensions: vec![LocExtension::varint(TIMESTAMP_ID, timestamp_micros)],
            payload: Bytes::from(data),
        })
        .map_err(|error| PyValueError::new_err(error.to_string()))
}
