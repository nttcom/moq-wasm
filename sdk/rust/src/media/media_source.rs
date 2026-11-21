use std::sync::{Arc, Mutex};

use broadcast::{Broadcast, MediaRendition, MediaSourceConfig, MediaSourceError, TrackKind};
use bytes::Bytes;
use mediapack::AudioLevel;
use moqt::publish::TrackDemand;
use tokio::sync::{Notify, mpsc};

use super::broadcast_end::BroadcastEnd;

#[derive(Debug, Clone)]
pub struct MediaSourceOptions {
    /// The video track, `video` by default; its media timeline is
    /// `<video>_timeline`.
    pub video: String,
    /// The audio track, `audio` by default.
    pub audio: String,
    /// The catalog's label of the video, and of the audio with ` audio`
    /// after it.
    pub label: String,
    /// The encoder's target in bits per second, for the catalog.
    pub video_bitrate: Option<u64>,
    /// The frames per second the encoder is capped at, for the catalog.
    pub video_framerate: Option<u32>,
    /// Has the relay ask for the tracks only while a subscriber takes them
    /// (draft-18 §10.10), reported by `MediaSource::demand_change`; `true`
    /// by default.
    pub on_demand: bool,
}

impl MediaSourceOptions {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            video: "video".into(),
            audio: "audio".into(),
            label: label.into(),
            video_bitrate: None,
            video_framerate: None,
            on_demand: true,
        }
    }
}

/// A video track with its renditions and an audio track, fed with encoded
/// frames and published as LOC with the MSF catalog and media timeline.
/// Once its broadcast is closed or dropped, its pushes fail and its waits
/// end with an error.
pub struct MediaSource {
    source: Mutex<broadcast::MediaSource>,
    end: BroadcastEnd,
    keyframe_requests: Arc<Notify>,
    /// `None` for a source not published on demand.
    demand_receiver: Option<mpsc::UnboundedReceiver<TrackDemand>>,
}

impl MediaSource {
    pub(super) fn new(
        broadcast: &Broadcast,
        options: MediaSourceOptions,
        end: BroadcastEnd,
    ) -> Self {
        let config = MediaSourceConfig {
            video: options.video,
            audio: options.audio,
            label: options.label,
            video_bitrate: options.video_bitrate,
            video_framerate: options.video_framerate,
            cmaf: false,
            on_demand: options.on_demand,
        };
        let mut source = broadcast::MediaSource::new(broadcast.clone(), config);
        Self {
            keyframe_requests: source.keyframe_requests(),
            demand_receiver: source.take_demand_receiver(),
            source: Mutex::new(source),
            end,
        }
    }

    /// `codec` is a WebCodecs codec string: `avc1.*` or `avc3.*` and
    /// `hvc1.*` or `hev1.*` in Annex B, `vp8`, `vp09.*` or `av01.*`.
    pub fn configure_video(&self, codec: &str) -> Result<(), MediaSourceError> {
        self.with(|source| source.configure_video(codec))
    }

    /// One encoded frame, after `configure_video`. A keyframe opens a group
    /// and sets the track's configuration (for H.264 and H.265, its
    /// parameter sets). `timestamp_micros` must not run backwards.
    pub fn push_video(
        &self,
        data: impl Into<Bytes>,
        timestamp_micros: u64,
    ) -> Result<(), MediaSourceError> {
        self.with(|source| source.push_video(data.into(), timestamp_micros))
    }

    /// `codec` is `opus`, or an AAC `mp4a.40.*` whose `description` is the
    /// AudioSpecificConfig.
    pub fn configure_audio(
        &self,
        codec: &str,
        sample_rate: u32,
        channels: u8,
        description: Option<&[u8]>,
    ) -> Result<(), MediaSourceError> {
        self.with(|source| source.configure_audio(codec, sample_rate, channels, description))
    }

    /// One encoded audio frame, after `configure_audio`; `level` is written
    /// as its LOC Audio Level property.
    pub fn push_audio(
        &self,
        data: impl Into<Bytes>,
        timestamp_micros: u64,
        level: Option<AudioLevel>,
    ) -> Result<(), MediaSourceError> {
        self.with(|source| source.push_audio(data.into(), timestamp_micros, level))
    }

    /// Another encoding of the video in the track `<video>_<name>`: feed it
    /// frames of the same pictures, with keyframes on the same pictures as
    /// the video's. It joins the video's switching set
    /// (draft-ietf-moq-cmsf-01 §3.2). `bitrate` (bits per second) and
    /// `framerate` go into the catalog.
    pub fn add_rendition(
        &self,
        name: &str,
        label: impl Into<String>,
        bitrate: Option<u64>,
        framerate: Option<u32>,
    ) -> Result<Rendition, MediaSourceError> {
        let rendition =
            self.with(|source| Ok(source.add_rendition(name, label.into(), bitrate, framerate)))?;
        Ok(Rendition {
            rendition,
            end: self.end.clone(),
        })
    }

    /// Ends the video (with its renditions) or the audio with PUBLISH_DONE
    /// and sends the catalog without it. Stop pushing its media first;
    /// configuring it again and pushing publishes it again. Fails when the
    /// relay session has ended.
    pub async fn remove_track(&self, kind: TrackKind) -> anyhow::Result<()> {
        let removal = self.with(|source| Ok(source.remove_track(kind)))?;
        removal.await
    }

    /// Resolves the next time a subscriber asks for a new group
    /// (NEW_GROUP_REQUEST) or the broadcast needs one: encode the next frame
    /// of the video and of every rendition as a keyframe then. It does not
    /// end with the session: wait for `Session::closed` beside it.
    pub async fn keyframe_request(&self) -> anyhow::Result<()> {
        self.end.until(self.keyframe_requests.notified()).await
    }

    /// The next change of whether a subscriber takes one of the source's
    /// tracks. Fails for a source added without `on_demand`.
    pub async fn demand_change(&mut self) -> anyhow::Result<TrackDemand> {
        let demand_receiver = self
            .demand_receiver
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("the source was added without on_demand"))?;
        self.end
            .until(demand_receiver.recv())
            .await?
            .ok_or_else(|| anyhow::anyhow!("the media source stopped publishing"))
    }

    fn with<R>(
        &self,
        call: impl FnOnce(&mut broadcast::MediaSource) -> Result<R, MediaSourceError>,
    ) -> Result<R, MediaSourceError> {
        self.end.check()?;
        call(&mut self.source.lock().unwrap())
    }
}

/// Another encoding of a `MediaSource`'s video.
pub struct Rendition {
    rendition: MediaRendition,
    end: BroadcastEnd,
}

impl Rendition {
    /// As `MediaSource::configure_video`; it may differ from the video's
    /// codec.
    pub fn configure_video(&mut self, codec: &str) -> Result<(), MediaSourceError> {
        self.end.check()?;
        self.rendition.configure_video(codec)
    }

    /// An encoded frame of the same picture as a video frame, at that
    /// frame's timestamp.
    pub fn push_video(
        &mut self,
        data: impl Into<Bytes>,
        timestamp_micros: u64,
    ) -> Result<(), MediaSourceError> {
        self.end.check()?;
        self.rendition.push_video(data.into(), timestamp_micros)
    }
}
