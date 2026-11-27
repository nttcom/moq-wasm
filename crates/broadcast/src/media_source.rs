mod demand_changes;
mod rendition_task;

use std::{fmt, future::Future, sync::Arc};

use bytes::Bytes;
use mediapack::{
    AudioConfig, AudioLevel, AudioSample, MediaEvent, Timestamp,
    aac::AudioSpecificConfig,
    loc::{Demuxer as LocDemuxer, LocExtension, LocObject, TIMESTAMP_ID, VideoCodec},
};
use tokio::sync::{Notify, mpsc, oneshot};

use crate::{
    broadcast::{Broadcast, SourceTracks, timeline_track_name},
    publish_queue::PushOutcome,
    rendition_track::{RenditionConfig, RenditionKeyframes, RenditionTrack},
    writer::{BroadcastWriter, remove_track_after},
};
pub use demand_changes::DemandChanges;
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
    /// (draft-18 §10.10), reported through `MediaSource::demand_changes`.
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

/// Publishes pre-encoded video/audio LOC tracks, renditions, catalog, and media timeline to a `Broadcast`.
/// `new` spawns a publish task; call inside `moqt::executor`'s runtime (Tokio on native).
pub struct MediaSource {
    writer: BroadcastWriter,
    /// Reads a frame as it reads a received LOC object: into the track's
    /// configuration changes and a sample.
    video: Option<LocDemuxer>,
    keyframe_requests: Arc<Notify>,
    demand_changes: Option<DemandChanges>,
    renditions: Vec<RenditionCommands>,
}

/// `track_name` lets `end` remove the track itself once the task has stopped.
struct RenditionCommands {
    track_name: String,
    command_sender: mpsc::Sender<RenditionCommand>,
}

impl MediaSource {
    pub fn new(broadcast: Broadcast, config: MediaSourceConfig) -> Self {
        let keyframe_requests = Arc::new(Notify::new());
        let (demand_sender, demand_changes) = config
            .on_demand
            .then(|| {
                let (demand_sender, demand_receiver) = mpsc::unbounded_channel();
                (demand_sender, DemandChanges::run(demand_receiver))
            })
            .unzip();
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
            demand_changes,
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
    ) -> Result<PushOutcome, MediaSourceError> {
        let events = demux(self.video.as_mut(), data, timestamp_micros)?;
        push_frame(events, |event| self.push(event))
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
        self.push(MediaEvent::AudioConfig(config)).map(drop)
    }

    /// One encoded audio frame, after `configure_audio`. `level` is its
    /// LOC Audio Level property.
    pub fn push_audio(
        &mut self,
        data: Bytes,
        timestamp_micros: u64,
        level: Option<AudioLevel>,
    ) -> Result<PushOutcome, MediaSourceError> {
        self.push(MediaEvent::Audio(AudioSample {
            data,
            pts: Timestamp::from_micros(timestamp_micros),
            audio_level: level,
        }))
    }

    /// Add a video rendition `<video>_<name>` in the video's switching set (`draft-ietf-moq-cmsf-01` §3.2).
    /// Feed the same pictures and keyframe positions as the primary video (`bitrate` in bits/s).
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
            track_name.clone(),
            self.keyframe_requests.clone(),
        );
        self.renditions.push(RenditionCommands {
            track_name,
            command_sender: task.command_sender(),
        });
        MediaRendition { task, video: None }
    }

    /// End video (plus renditions) or audio with `PUBLISH_DONE` and update the catalog.
    /// Stop pushing first; re-configure and push to resume.
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
            TrackKind::Video => self
                .renditions
                .iter()
                .map(|rendition| remove_rendition(&rendition.command_sender))
                .collect(),
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

    /// Ends the video, its renditions and the audio with PUBLISH_DONE after
    /// the media queued before, as `remove_track` does; later pushes to its
    /// renditions fail with `MediaSourceError::Stopped`. A PUBLISH of these
    /// tracks or the media timeline still waiting for the relay's answer is
    /// withdrawn first, which stops the task waiting on it and drops the
    /// media queued behind. A track whose publish task stopped is removed
    /// from the broadcast directly. A step that fails, as once the relay
    /// session ended, does not stop the others; the first failure is
    /// returned.
    pub fn end(self) -> impl Future<Output = anyhow::Result<()>> + use<> {
        let tracks = self.writer.source_tracks();
        let broadcast = self.writer.broadcast().clone();
        let mut withdrawn = vec![
            tracks.video.clone(),
            timeline_track_name(&tracks.video),
            tracks.audio.clone(),
        ];
        withdrawn.extend(
            self.renditions
                .iter()
                .map(|rendition| rendition.track_name.clone()),
        );
        let video = self.writer.end_track(&tracks.video);
        let renditions: Vec<_> = self
            .renditions
            .iter()
            .map(|rendition| end_rendition(rendition, broadcast.clone()))
            .collect();
        let audio = self.writer.end_track(&tracks.audio);
        async move {
            broadcast
                .connection()
                .withdraw_publishes(broadcast.namespace(), &withdrawn)
                .await;
            let mut result = video.await;
            for ended in renditions {
                result = result.and(ended.await);
            }
            result.and(audio.await)
        }
    }

    /// Notified when a subscriber asks for a new group (NEW_GROUP_REQUEST)
    /// or the broadcast needs one: encode the next frame of the video and of
    /// every rendition as a keyframe then.
    pub fn keyframe_requests(&self) -> Arc<Notify> {
        self.keyframe_requests.clone()
    }

    /// The changes of whether a subscriber takes one of the source's tracks,
    /// CMAF siblings included; `None` without `on_demand`.
    pub fn demand_changes(&self) -> Option<DemandChanges> {
        self.demand_changes.clone()
    }

    fn push(&mut self, event: MediaEvent) -> Result<PushOutcome, MediaSourceError> {
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
        let events = demux(self.video.as_mut(), data, timestamp_micros)?;
        push_frame(events, |event| {
            self.task.push(event).map_err(MediaSourceError::Stopped)
        })
    }
}

/// The outcome of the frame's video sample, or `NoPicture` when the demuxer
/// read none; the configurations demuxed with it are not reported.
fn push_frame(
    events: Vec<MediaEvent>,
    mut push: impl FnMut(MediaEvent) -> Result<PushOutcome, MediaSourceError>,
) -> Result<PushOutcome, MediaSourceError> {
    let mut outcome = PushOutcome::NoPicture;
    for event in events {
        let is_frame = matches!(event, MediaEvent::Video(_));
        let pushed = push(event)?;
        if is_frame {
            outcome = pushed;
        }
    }
    Ok(outcome)
}

fn remove_rendition(
    command_sender: &mpsc::Sender<RenditionCommand>,
) -> impl Future<Output = anyhow::Result<()>> + use<> {
    let removal = send_rendition_command(command_sender, RenditionCommand::Remove);
    async move {
        removal
            .await
            .ok_or_else(|| anyhow::anyhow!("rendition task ended"))?
    }
}

fn end_rendition(
    rendition: &RenditionCommands,
    broadcast: Broadcast,
) -> impl Future<Output = anyhow::Result<()>> + use<> {
    remove_track_after(
        send_rendition_command(&rendition.command_sender, RenditionCommand::End),
        broadcast,
        rendition.track_name.clone(),
    )
}

/// Waits for room in the rendition's queue rather than dropping the command,
/// which is never repeated. `None` when the task has stopped and so never
/// takes the command.
fn send_rendition_command(
    command_sender: &mpsc::Sender<RenditionCommand>,
    command: fn(oneshot::Sender<anyhow::Result<()>>) -> RenditionCommand,
) -> impl Future<Output = Option<anyhow::Result<()>>> + use<> {
    let command_sender = command_sender.clone();
    async move {
        let (done_sender, done_receiver) = oneshot::channel();
        command_sender.send(command(done_sender)).await.ok()?;
        done_receiver.await.ok()
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
    use moqt::{Subscription, Version};

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
    async fn only_a_source_on_demand_has_demand_changes() {
        // Arrange
        let on_demand = source_on(RelayConnection::new(None), true);
        let always_sent = source_on(RelayConnection::new(None), false);

        // Act
        let on_demand_changes = on_demand.demand_changes();
        let always_sent_changes = always_sent.demand_changes();

        // Assert
        assert!(on_demand_changes.is_some());
        assert!(always_sent_changes.is_none());
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
        let outcome = source
            .push_audio(Bytes::from_static(b"opus"), 0, None)
            .unwrap();
        let object = relay.first_track_object("audio").await;

        // Assert
        assert_eq!(outcome, PushOutcome::Queued);
        assert_eq!(object.payload.as_ref(), b"opus");
    }

    #[tokio::test]
    async fn push_video_reports_the_frames_the_full_publish_queue_drops() {
        // Arrange: without an await the publish task never takes a frame, as while the relay stalls
        let mut source = source();
        source.configure_video("vp8").unwrap();

        // Act
        let outcomes: Vec<_> = (0..3_000)
            .map(|frame| source.push_video(vp8_keyframe(), frame * 33_333).unwrap())
            .collect();

        // Assert
        let queued = outcomes
            .iter()
            .take_while(|outcome| **outcome == PushOutcome::Queued)
            .count();
        assert!(queued > 0 && queued < outcomes.len(), "{queued}");
        assert!(
            outcomes[queued..]
                .iter()
                .all(|outcome| *outcome == PushOutcome::Dropped)
        );
    }

    #[tokio::test]
    async fn a_frame_without_a_slice_has_no_picture() {
        // Arrange
        let mut source = source();
        source.configure_video("avc1.42001f").unwrap();
        let access_unit_delimiter = Bytes::from_static(&[0, 0, 0, 1, 0x09, 0xf0]);

        // Act
        let outcome = source.push_video(access_unit_delimiter, 0);

        // Assert
        assert_eq!(outcome.unwrap(), PushOutcome::NoPicture);
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
    async fn ending_a_source_ends_its_tracks_after_the_media_queued_before() {
        // Arrange: nothing is sent before the end, since the publish task is still connecting
        let mut relay = FakeRelay::spawn("media-source-end");
        let mut source = source_on(relay.connection(), false);
        let mut rendition = source.add_rendition("low", "low".to_string(), None, None);
        source.configure_video("vp8").unwrap();
        source.push_video(vp8_keyframe(), 0).unwrap();
        rendition.configure_video("vp8").unwrap();
        rendition.push_video(vp8_keyframe(), 0).unwrap();
        source.configure_audio("opus", 48_000, 1, None).unwrap();
        for index in 0..3u8 {
            source
                .push_audio(Bytes::from(vec![index]), u64::from(index) * 20_000, None)
                .unwrap();
        }

        // Act
        let ended = source.end().await;
        let before_done = relay
            .wait_for_publish_done("live", &["video", "video_low", "audio"])
            .await;
        let audio = relay.track_objects("audio", 3).await;
        let after_done = relay.received_requests();

        // Assert: the queued media was published before the end, and nothing publishes it again
        assert!(ended.is_ok(), "{ended:?}");
        for track in ["video", "video_low", "audio"] {
            assert!(
                before_done.contains(&format!("PUBLISH live/{track}")),
                "{before_done:?}"
            );
        }
        let payloads: Vec<&[u8]> = audio.iter().map(|object| object.payload.as_ref()).collect();
        assert_eq!(payloads, [[0], [1], [2]]);
        assert!(
            !after_done
                .iter()
                .any(|request| request.starts_with("PUBLISH ")),
            "{after_done:?}"
        );
    }

    #[tokio::test]
    async fn ending_a_source_withdraws_a_publish_the_relay_has_not_answered() {
        for version in [Version::Draft14, Version::Draft18] {
            // Arrange: the relay holds the audio PUBLISH unanswered
            let (answer_publish, publish_gate) = oneshot::channel();
            let mut relay = FakeRelay::spawn_holding_publish(
                "media-source-end-unanswered",
                version,
                "audio",
                publish_gate,
            );
            // Kept, so the session outlives the source and takes the late answer.
            let connection = relay.connection();
            let mut source = source_on(connection.clone(), false);
            source.configure_audio("opus", 48_000, 1, None).unwrap();
            source
                .push_audio(Bytes::from_static(b"a"), 0, None)
                .unwrap();
            relay.wait_for_request("PUBLISH live/audio").await;

            // Act
            let ended = tokio::time::timeout(TEST_TIMEOUT, source.end()).await;
            let _ = answer_publish.send(());

            // Assert: the PUBLISH answered late is ended
            assert!(matches!(ended, Ok(Ok(()))), "{version:?}: {ended:?}");
            relay.wait_for_publish_done("live", &["audio"]).await;
        }
    }

    #[tokio::test]
    async fn a_rendition_frame_after_its_source_ended_is_refused_as_stopped() {
        // Arrange
        let mut source = source();
        let mut rendition = source.add_rendition("low", "low".to_string(), None, None);
        rendition.configure_video("vp8").unwrap();
        source.end().await.unwrap();

        // Act
        let result = rendition.push_video(vp8_keyframe(), 0);

        // Assert
        assert!(matches!(result, Err(MediaSourceError::Stopped(_))));
    }

    #[tokio::test]
    async fn ending_a_source_ends_a_rendition_whose_task_stopped() {
        // Arrange: dropping the rendition once its frame arrived stops its task between frames,
        // so it takes no end
        let mut relay = FakeRelay::spawn("media-source-end-stopped-rendition");
        let mut source = source_on(relay.connection(), false);
        let mut rendition = source.add_rendition("low", "low".to_string(), None, None);
        rendition.configure_video("vp8").unwrap();
        rendition.push_video(vp8_keyframe(), 0).unwrap();
        relay.first_track_object("video_low").await;
        drop(rendition);

        // Act
        let ended = source.end().await;

        // Assert
        assert!(ended.is_ok(), "{ended:?}");
        relay.wait_for_publish_done("live", &["video_low"]).await;
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
