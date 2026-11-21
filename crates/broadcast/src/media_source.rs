mod rendition_task;

use std::{fmt, future::Future, sync::Arc};

use bytes::Bytes;
use mediapack::{
    AudioConfig, AudioLevel, AudioSample, MediaEvent, Timestamp,
    aac::AudioSpecificConfig,
    loc::{Demuxer as LocDemuxer, LocExtension, LocObject, TIMESTAMP_ID, VideoCodec},
};
use moqt::publish::TrackDemand;
use tokio::sync::{Notify, mpsc, oneshot};

use crate::{
    broadcast::{Broadcast, SourceTracks},
    publish_queue::PushOutcome,
    rendition_track::{RenditionConfig, RenditionKeyframes, RenditionTrack},
    writer::BroadcastWriter,
};
use rendition_task::{RenditionCommand, RenditionTask};

/// What a media source publishes its video and audio as.
pub struct MediaSourceConfig {
    pub video: String,
    pub audio: String,
    /// The video's label; the audio is labelled `<label> audio`.
    pub label: String,
    /// The video encoder's target in bits per second.
    pub video_bitrate: Option<u64>,
    /// The frame rate the video encoder is capped at.
    pub video_framerate: Option<u32>,
    pub cmaf: bool,
    /// Publishes the video, its renditions and the audio on demand
    /// (draft-18 §10.10), reported through `MediaSource::take_demand_receiver`.
    pub on_demand: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Video,
    Audio,
}

#[derive(Debug)]
#[non_exhaustive]
pub enum MediaSourceError {
    /// The codec, its configuration or the frame cannot be published.
    InvalidMedia(anyhow::Error),
    VideoNotConfigured,
    /// The source stopped publishing, as when its relay session ended.
    Stopped(anyhow::Error),
}

impl fmt::Display for MediaSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMedia(error) | Self::Stopped(error) => write!(formatter, "{error:#}"),
            Self::VideoNotConfigured => write!(formatter, "configure the video before its frames"),
        }
    }
}

impl std::error::Error for MediaSourceError {}

/// One source of a broadcast fed with frames encoded elsewhere, the shape
/// the browser and Python SDKs share: a video track with its renditions and
/// an audio track, published as LOC with the catalog and media timeline the
/// `BroadcastWriter` builds. Creating one spawns its publish task, so it
/// needs the runtime `moqt::executor` spawns on.
pub struct MediaSource {
    writer: BroadcastWriter,
    /// Reads a frame as it reads a received LOC object: into the track's
    /// configuration changes and a sample.
    video: Option<LocDemuxer>,
    keyframe_requests: Arc<Notify>,
    demand_receiver: Option<mpsc::UnboundedReceiver<TrackDemand>>,
    renditions: Vec<mpsc::Sender<RenditionCommand>>,
}

impl MediaSource {
    pub fn new(broadcast: Broadcast, config: MediaSourceConfig) -> Self {
        let keyframe_requests = Arc::new(Notify::new());
        let (demand_sender, demand_receiver) =
            config.on_demand.then(mpsc::unbounded_channel).unzip();
        let tracks = SourceTracks {
            switching_set: config.video.clone(),
            video: config.video,
            audio_label: format!("{} audio", config.label),
            video_label: config.label,
            audio: config.audio,
            cmaf: config.cmaf,
            video_bitrate: config.video_bitrate,
            video_framerate: config.video_framerate,
            keyframe_requests: Some(keyframe_requests.clone()),
            demand_sender,
        };
        Self {
            writer: BroadcastWriter::run(broadcast, tracks),
            video: None,
            keyframe_requests,
            demand_receiver,
            renditions: Vec::new(),
        }
    }

    /// `codec` is a WebCodecs codec string: `avc1.*` or `avc3.*` and
    /// `hvc1.*` or `hev1.*` in Annex B, `vp8`, `vp09.*` or `av01.*`.
    pub fn configure_video(&mut self, codec: &str) -> Result<(), MediaSourceError> {
        self.video = Some(video_demuxer(codec)?);
        Ok(())
    }

    /// One encoded frame. A keyframe opens a group and sets the track's
    /// configuration (for H.264 and H.265, its parameter sets).
    /// `timestamp_micros` must not run backwards.
    pub fn push_video(
        &mut self,
        data: Bytes,
        timestamp_micros: u64,
    ) -> Result<(), MediaSourceError> {
        for event in demux(self.video.as_mut(), data, timestamp_micros)? {
            self.push(event)?;
        }
        Ok(())
    }

    /// `codec` is `opus`, or an AAC `mp4a.40.*` whose `description` is the
    /// AudioSpecificConfig.
    pub fn configure_audio(
        &mut self,
        codec: &str,
        sample_rate: u32,
        channels: u8,
        description: Option<&[u8]>,
    ) -> Result<(), MediaSourceError> {
        let config = match codec {
            "opus" => AudioConfig::Opus {
                sample_rate,
                channels,
            },
            aac if aac.starts_with("mp4a.40.") => {
                let description = description.ok_or_else(|| {
                    MediaSourceError::InvalidMedia(anyhow::anyhow!(
                        "AAC needs its AudioSpecificConfig"
                    ))
                })?;
                AudioConfig::Aac(
                    AudioSpecificConfig::parse(description)
                        .map_err(MediaSourceError::InvalidMedia)?,
                )
            }
            other => {
                return Err(MediaSourceError::InvalidMedia(anyhow::anyhow!(
                    "unsupported audio codec: {other}"
                )));
            }
        };
        self.push(MediaEvent::AudioConfig(config))
    }

    /// One encoded audio frame, after `configure_audio`. `level` is its
    /// LOC Audio Level property.
    pub fn push_audio(
        &mut self,
        data: Bytes,
        timestamp_micros: u64,
        level: Option<AudioLevel>,
    ) -> Result<(), MediaSourceError> {
        self.push(MediaEvent::Audio(AudioSample {
            data,
            pts: Timestamp::from_micros(timestamp_micros),
            audio_level: level,
        }))
    }

    /// A rendition of the video in the track `<video>_<name>`, whose groups
    /// start on the same frames and take the same group ids as the video's
    /// (draft-ietf-moq-cmsf-01 §3.2). It joins the video's switching set, so
    /// a rendition in another codec is an alternative to the video.
    pub fn add_rendition(
        &mut self,
        name: &str,
        label: String,
        bitrate: Option<u64>,
        framerate: Option<u32>,
    ) -> MediaRendition {
        let track_name = format!("{}_{name}", self.writer.source_tracks().video);
        let config = RenditionConfig {
            track: track_name.clone(),
            label,
            bitrate,
            framerate,
            cmaf: self.writer.source_tracks().cmaf,
            keyframes: RenditionKeyframes::SameFrames,
        };
        let track = RenditionTrack::new(&self.writer, config);
        let task = RenditionTask::run(
            track,
            self.writer.broadcast().clone(),
            track_name,
            self.keyframe_requests.clone(),
        );
        self.renditions.push(task.command_sender());
        MediaRendition { task, video: None }
    }

    /// Ends the video or the audio with PUBLISH_DONE and sends the catalog
    /// without it; removing the video removes its renditions. Stop pushing
    /// its media first: configuring it again and pushing publishes it again.
    /// The future does not borrow the source, so the other kind can be
    /// pushed meanwhile.
    pub fn remove_track(
        &self,
        kind: TrackKind,
    ) -> impl Future<Output = anyhow::Result<()>> + use<> {
        let tracks = self.writer.source_tracks();
        let track_name = match kind {
            TrackKind::Video => &tracks.video,
            TrackKind::Audio => &tracks.audio,
        };
        let removal = self.writer.remove_track(track_name);
        let renditions: Vec<_> = match kind {
            TrackKind::Video => self.renditions.iter().map(remove_rendition).collect(),
            TrackKind::Audio => Vec::new(),
        };
        async move {
            let mut result = removal.await;
            for removed in renditions {
                result = result.and(removed.await);
            }
            result
        }
    }

    /// Notified when a subscriber asks for a new group (NEW_GROUP_REQUEST)
    /// or the broadcast needs one: encode the next frame of the video and of
    /// every rendition as a keyframe then.
    pub fn keyframe_requests(&self) -> Arc<Notify> {
        self.keyframe_requests.clone()
    }

    /// Each change of whether a subscriber takes one of the source's tracks,
    /// CMAF siblings included; `None` without `on_demand` or once taken. The
    /// channel closes once every track of the source is gone.
    pub fn take_demand_receiver(&mut self) -> Option<mpsc::UnboundedReceiver<TrackDemand>> {
        self.demand_receiver.take()
    }

    fn push(&mut self, event: MediaEvent) -> Result<(), MediaSourceError> {
        self.writer.push(event).map_err(MediaSourceError::Stopped)
    }
}

/// One rendition of a `MediaSource`'s video, fed with frames of the same
/// pictures as the video. Dropping it stops its task.
pub struct MediaRendition {
    task: RenditionTask,
    video: Option<LocDemuxer>,
}

impl MediaRendition {
    /// As `MediaSource::configure_video`.
    pub fn configure_video(&mut self, codec: &str) -> Result<(), MediaSourceError> {
        self.video = Some(video_demuxer(codec)?);
        Ok(())
    }

    /// The encoded frame of the same picture as a video frame, at that
    /// frame's timestamp. While the relay does not keep up, up to 296 frames
    /// wait in the queue; later frames are dropped up to the next keyframe
    /// that fits (`PushOutcome::Dropped`). Fails once an earlier frame
    /// stopped the rendition.
    pub fn push_video(
        &mut self,
        data: Bytes,
        timestamp_micros: u64,
    ) -> Result<PushOutcome, MediaSourceError> {
        let mut outcome = PushOutcome::NoPicture;
        for event in demux(self.video.as_mut(), data, timestamp_micros)? {
            let is_frame = matches!(event, MediaEvent::Video(_));
            let pushed = self.task.push(event).map_err(MediaSourceError::Stopped)?;
            if is_frame {
                outcome = pushed;
            }
        }
        Ok(outcome)
    }
}

/// Waits for room in the rendition's queue rather than dropping the removal,
/// which is never repeated.
fn remove_rendition(
    command_sender: &mpsc::Sender<RenditionCommand>,
) -> impl Future<Output = anyhow::Result<()>> + use<> {
    let command_sender = command_sender.clone();
    async move {
        let (done_sender, done_receiver) = oneshot::channel();
        command_sender
            .send(RenditionCommand::Remove(done_sender))
            .await
            .map_err(|_| anyhow::anyhow!("rendition task ended"))?;
        done_receiver
            .await
            .map_err(|_| anyhow::anyhow!("rendition task ended"))?
    }
}

fn video_demuxer(codec: &str) -> Result<LocDemuxer, MediaSourceError> {
    let video_codec = VideoCodec::from_codec_string(codec).ok_or_else(|| {
        MediaSourceError::InvalidMedia(anyhow::anyhow!("unsupported video codec: {codec}"))
    })?;
    Ok(LocDemuxer::video(video_codec).with_capture_origin(Timestamp::ZERO))
}

fn demux(
    video: Option<&mut LocDemuxer>,
    data: Bytes,
    timestamp_micros: u64,
) -> Result<Vec<MediaEvent>, MediaSourceError> {
    let video = video.ok_or(MediaSourceError::VideoNotConfigured)?;
    video
        .push(&LocObject {
            extensions: vec![LocExtension::varint(TIMESTAMP_ID, timestamp_micros)],
            payload: data,
        })
        .map_err(MediaSourceError::InvalidMedia)
}

#[cfg(test)]
mod tests {
    use moqt::Subscription;

    use super::*;
    use crate::{
        broadcast::VIDEO_TRACK_NAME,
        relay_connection::RelayConnection,
        test_support::{FakeRelay, TEST_TIMEOUT},
    };

    fn source_on(connection: RelayConnection, on_demand: bool) -> MediaSource {
        let broadcast = Broadcast::new(connection, vec!["live".to_string()]);
        MediaSource::new(
            broadcast,
            MediaSourceConfig {
                video: "video".to_string(),
                audio: "audio".to_string(),
                label: "camera".to_string(),
                video_bitrate: None,
                video_framerate: None,
                cmaf: false,
                on_demand,
            },
        )
    }

    fn source() -> MediaSource {
        source_on(RelayConnection::new(None), true)
    }

    fn vp8_keyframe() -> Bytes {
        Bytes::from_static(&[0x00, 0x00, 0x00, 0x9d, 0x01, 0x2a, 64, 0, 48, 0])
    }

    #[tokio::test]
    async fn a_frame_before_the_video_configuration_is_refused() {
        // Arrange
        let mut source = source();

        // Act
        let result = source.push_video(Bytes::from_static(b"frame"), 0);

        // Assert
        assert!(matches!(result, Err(MediaSourceError::VideoNotConfigured)));
    }

    #[tokio::test]
    async fn a_codec_the_broadcast_does_not_publish_is_invalid_media() {
        // Arrange
        let mut source = source();

        // Act
        let video = source.configure_video("h264");
        let audio = source.configure_audio("aac", 48_000, 1, None);

        // Assert
        assert!(matches!(video, Err(MediaSourceError::InvalidMedia(_))));
        assert!(matches!(audio, Err(MediaSourceError::InvalidMedia(_))));
    }

    #[tokio::test]
    async fn aac_needs_an_audio_specific_config_that_parses() {
        // Arrange
        let mut source = source();

        // Act
        let missing = source.configure_audio("mp4a.40.2", 48_000, 1, None);
        let invalid = source.configure_audio("mp4a.40.2", 48_000, 1, Some(&[0xff]));

        // Assert
        assert!(matches!(missing, Err(MediaSourceError::InvalidMedia(_))));
        assert!(matches!(invalid, Err(MediaSourceError::InvalidMedia(_))));
    }

    #[tokio::test]
    async fn the_demand_receiver_is_taken_once() {
        // Arrange
        let mut source = source();

        // Act
        let first = source.take_demand_receiver();
        let second = source.take_demand_receiver();

        // Assert
        assert!(first.is_some());
        assert!(second.is_none());
    }

    #[tokio::test]
    async fn a_rendition_frame_before_its_configuration_is_refused() {
        // Arrange
        let mut source = source();
        let mut rendition = source.add_rendition("low", "low".to_string(), None, None);

        // Act
        let result = rendition.push_video(Bytes::from_static(b"frame"), 0);

        // Assert
        assert!(matches!(result, Err(MediaSourceError::VideoNotConfigured)));
    }

    #[tokio::test]
    async fn a_removal_outlives_the_borrow_of_its_source() {
        // Arrange
        let source = std::sync::Mutex::new(source());

        // Act: the future leaves the lock, as an SDK holding the source behind one awaits it
        let removal = source.lock().unwrap().remove_track(TrackKind::Audio);
        let removed = removal.await;

        // Assert: without a relay connection nothing is sent
        assert!(removed.is_ok(), "{removed:?}");
    }

    #[tokio::test]
    async fn aac_accepts_mp4a_40_with_a_valid_audio_specific_config() {
        // Arrange
        let mut source = source();
        let description = AudioSpecificConfig::new(2, 48_000, 1).to_bytes();

        // Act
        let result = source.configure_audio("mp4a.40.2", 48_000, 1, Some(&description));

        // Assert
        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    async fn an_unsupported_audio_codec_with_an_audio_specific_config_is_invalid_media() {
        // Arrange
        let mut source = source();
        let description = AudioSpecificConfig::new(2, 48_000, 1).to_bytes();

        // Act
        let result = source.configure_audio("aac", 48_000, 1, Some(&description));

        // Assert
        assert!(matches!(result, Err(MediaSourceError::InvalidMedia(_))));
    }

    #[tokio::test]
    async fn push_audio_sends_the_audio_object_to_the_relay() {
        // Arrange
        let mut relay = FakeRelay::spawn("media-source-audio");
        let mut source = source_on(relay.connection(), false);
        source.configure_audio("opus", 48_000, 1, None).unwrap();

        // Act
        source
            .push_audio(Bytes::from_static(b"opus"), 0, None)
            .unwrap();
        let object = relay.first_track_object("audio").await;

        // Assert
        assert_eq!(object.payload.as_ref(), b"opus");
    }

    #[tokio::test]
    async fn a_new_group_request_notifies_the_source_s_keyframe_requests() {
        // Arrange
        let mut relay = FakeRelay::spawn_draft18("media-source-new-group-request");
        let mut source = source_on(relay.connection(), false);
        let keyframe_requests = source.keyframe_requests();
        source.configure_video("vp8").unwrap();
        source.push_video(vp8_keyframe(), 0).unwrap();
        let video = tokio::time::timeout(TEST_TIMEOUT, async {
            loop {
                let published = relay.published.recv().await.unwrap();
                if matches!(&published, Subscription::PublisherInitiated(published) if published.track_name == VIDEO_TRACK_NAME)
                {
                    return published;
                }
            }
        })
        .await
        .expect("the video must be published");
        let relay_session = relay.session().await;

        // Act
        let sent = relay_session
            .subscriber()
            .request_new_group(&video, None)
            .await
            .unwrap();

        // Assert
        assert!(sent, "the video must be published with dynamic groups");
        assert!(
            tokio::time::timeout(TEST_TIMEOUT, keyframe_requests.notified())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_rendition_codec_the_broadcast_does_not_publish_is_invalid_media() {
        // Arrange
        let mut source = source();
        let mut rendition = source.add_rendition("low", "low".to_string(), None, None);

        // Act
        let result = rendition.configure_video("h264");

        // Assert
        assert!(matches!(result, Err(MediaSourceError::InvalidMedia(_))));
    }

    #[tokio::test]
    async fn a_rendition_sends_its_configured_video_to_the_relay() {
        // Arrange
        let mut relay = FakeRelay::spawn("media-source-rendition");
        let mut source = source_on(relay.connection(), false);
        let mut rendition = source.add_rendition("low", "low".to_string(), None, None);
        let frame = vp8_keyframe();

        // Act
        rendition.configure_video("vp8").unwrap();
        let outcome = rendition.push_video(frame.clone(), 0).unwrap();
        let object = relay.first_track_object("video_low").await;

        // Assert
        assert_eq!(outcome, PushOutcome::Queued);
        assert_eq!(object.payload, frame);
    }

    #[tokio::test]
    async fn a_rendition_frame_without_a_slice_has_no_picture() {
        // Arrange
        let mut source = source();
        let mut rendition = source.add_rendition("low", "low".to_string(), None, None);
        rendition.configure_video("avc1.42001f").unwrap();
        let access_unit_delimiter = Bytes::from_static(&[0, 0, 0, 1, 0x09, 0xf0]);

        // Act
        let outcome = rendition.push_video(access_unit_delimiter, 0);

        // Assert
        assert_eq!(outcome.unwrap(), PushOutcome::NoPicture);
    }
}
