use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use bytes::Bytes;
use mediapack::{
    AudioConfig, AudioSample, MediaEvent, Timestamp, VideoSample,
    loc::{Muxer as LocMuxer, VideoCodec, to_extension_headers},
    mp4::Fmp4TrackMuxer,
};
use moqt::{
    executor,
    publish::{GroupBoundary, OutgoingObject},
};

use crate::{
    broadcast::{
        AudioTrackInfo, Broadcast, SourceTracks, VideoTrackInfo, cmaf_track_name,
        timeline_track_name,
    },
    cmaf_video::CmafVideo,
    group_alignment::{GroupAlignment, StartedGroups},
    media_timeline::MediaTimeline,
};

/// One DEBUG line per sample as it enters (`ingest`) and as it is sent to the
/// relay (`publish`, with the group id and LOC capture timestamp a viewer
/// sees), so that what a viewer received can be checked against
/// what was sent: `RUST_LOG=broadcast::delivery=debug`.
const DELIVERY_LOG_TARGET: &str = "broadcast::delivery";
/// A late joiner starts at a group, so this bounds how far it joins behind the
/// live edge.
const AUDIO_ONLY_GROUP_DURATION_US: u64 = 1_000_000;

/// The clocks every track of one switching set stamps from, so renditions of
/// the source video carry the same capture timestamp and group id for the same
/// presentation time (draft-ietf-moq-cmsf-01 §3.2).
#[derive(Clone)]
pub(crate) struct SharedTiming {
    /// The wall-clock time of presentation time zero.
    capture_origin: Arc<OnceLock<Timestamp>>,
    pub(crate) alignment: Arc<GroupAlignment>,
}

impl SharedTiming {
    fn new() -> Self {
        Self {
            capture_origin: Arc::new(OnceLock::new()),
            alignment: Arc::new(GroupAlignment::new(wall_clock().micros())),
        }
    }

    /// The first call takes the wall clock as the capture time of
    /// `presentation_time`; later calls ignore it.
    pub(crate) fn capture_origin(&self, presentation_time: Timestamp) -> Timestamp {
        *self
            .capture_origin
            .get_or_init(|| wall_clock().saturating_sub(presentation_time))
    }

    pub(crate) fn seeded_capture_origin(&self) -> Option<Timestamp> {
        self.capture_origin.get().copied()
    }
}

pub(crate) struct TrackPublisher {
    broadcast: Broadcast,
    tracks: SourceTracks,
    announced: bool,
    /// The group the audio is being written into: the latest video keyframe
    /// group, so the audio tracks start a group when the video tracks do, or,
    /// while the source publishes no video, one the audio started itself.
    audio_group_id: Option<u64>,
    audio_only_group_start_us: Option<u64>,
    /// Between the video's configuration and its removal.
    video_published: bool,
    audio_config: Option<AudioConfig>,
    timeline: MediaTimeline,
    timing: SharedTiming,
    video_codec: VideoCodec,
    video_groups: StartedGroups,
    video_cmaf: Option<CmafVideo>,
    audio_cmaf: Option<Fmp4TrackMuxer>,
}

impl TrackPublisher {
    pub(crate) fn new(broadcast: Broadcast, tracks: SourceTracks) -> Self {
        Self {
            broadcast,
            tracks,
            announced: false,
            audio_group_id: None,
            audio_only_group_start_us: None,
            video_published: false,
            audio_config: None,
            timeline: MediaTimeline::default(),
            timing: SharedTiming::new(),
            video_codec: VideoCodec::H264,
            video_groups: StartedGroups::default(),
            video_cmaf: None,
            audio_cmaf: None,
        }
    }

    pub(crate) async fn remove_track(&mut self, track_name: &str) -> Result<()> {
        if track_name == self.tracks.video {
            self.video_published = false;
        }
        // Audio published again starts a group at once instead of writing into
        // the group the removal ended.
        if track_name == self.tracks.audio {
            self.audio_only_group_start_us = None;
        }
        self.broadcast.remove_track(track_name).await
    }

    pub(crate) fn shared_timing(&self) -> SharedTiming {
        self.timing.clone()
    }

    pub(crate) async fn push(&mut self, event: &MediaEvent) -> Result<()> {
        match event {
            MediaEvent::Streams(_) => Ok(()),
            MediaEvent::VideoConfig(config) => {
                self.video_published = true;
                self.video_codec = VideoCodec::from(config);
                self.video_cmaf = self.tracks.cmaf.then(|| CmafVideo::new(config.clone()));
                let info = VideoTrackInfo {
                    cmaf: self.tracks.cmaf,
                    switching_set: self.tracks.switching_set.clone(),
                    source: self.tracks.video.clone(),
                    bitrate: self.tracks.video_bitrate,
                    framerate: self.tracks.video_framerate,
                    ..VideoTrackInfo::from_config(config, self.tracks.video_label.clone())?
                };
                self.broadcast
                    .prepare_encoded_video(
                        &self.tracks.video,
                        self.tracks.cmaf,
                        self.tracks.keyframe_requests.as_ref(),
                        self.tracks.demand_sender.as_ref(),
                    )
                    .await?;
                self.broadcast
                    .update_video_catalog(&self.tracks.video, info)
                    .await
            }
            MediaEvent::AudioConfig(config) => {
                self.audio_cmaf = match config {
                    AudioConfig::Aac(_) if self.tracks.cmaf => {
                        Some(Fmp4TrackMuxer::audio(config.clone()))
                    }
                    _ => None,
                };
                let info = AudioTrackInfo {
                    label: self.tracks.audio_label.clone(),
                    config: config.clone(),
                    cmaf: self.tracks.cmaf,
                    switching_set: self.tracks.switching_set.clone(),
                    source: self.tracks.video.clone(),
                };
                if let Some(demand_sender) = &self.tracks.demand_sender {
                    self.broadcast
                        .publish_on_demand(&self.tracks.audio, demand_sender.clone())
                        .await?;
                }
                self.broadcast
                    .update_audio_catalog(&self.tracks.audio, info)
                    .await?;
                self.audio_config = Some(config.clone());
                // A track published while a group is open is sent from its
                // next group, which the audio starts at the next video
                // keyframe.
                if self.video_published
                    && let Some(requested) = &self.tracks.keyframe_requests
                {
                    requested.notify_one();
                }
                Ok(())
            }
            MediaEvent::Video(sample) => self.publish_video(sample).await,
            MediaEvent::Audio(sample) => self.publish_audio(sample).await,
        }
    }

    async fn publish_video(&mut self, sample: &VideoSample) -> Result<()> {
        self.announce().await?;
        tracing::debug!(
            target: DELIVERY_LOG_TARGET,
            stage = "ingest",
            namespace = %self.broadcast.namespace().join("/"),
            track = %self.tracks.video,
            pts_us = sample.pts.micros(),
            bytes = sample.data.len(),
            keyframe = sample.is_keyframe
        );
        let Some(object) = LocMuxer::new(self.timing.capture_origin(sample.pts))
            .with_video_codec(self.video_codec)
            .push(&MediaEvent::Video(sample.clone()))
        else {
            return Ok(());
        };
        let captured_at = object.capture_timestamp();
        let bytes = object.payload.len();
        let group = self.video_groups.boundary(
            sample
                .is_keyframe
                .then(|| self.timing.alignment.keyframe(sample.pts.micros())),
        );
        let written = self
            .broadcast
            .send_object(
                &self.tracks.video,
                OutgoingObject {
                    group,
                    extension_headers: to_extension_headers(&object.extensions),
                    payload: object.payload,
                },
            )
            .await?;
        if written {
            tracing::debug!(
                target: DELIVERY_LOG_TARGET,
                stage = "publish",
                namespace = %self.broadcast.namespace().join("/"),
                track = %self.tracks.video,
                group_id = self
                    .timing
                    .alignment
                    .group_covering(sample.pts.micros())
                    .unwrap_or_default(),
                capture_us = captured_at.map_or(0, |at| at.micros()),
                pts_us = sample.pts.micros(),
                bytes,
                keyframe = sample.is_keyframe
            );
        }
        self.publish_cmaf_video(sample, group).await?;
        let (GroupBoundary::At(group_id), Some(captured_at)) = (group, captured_at) else {
            return Ok(());
        };
        self.publish_timeline(group_id, sample.pts.micros(), captured_at.millis())
            .await
    }

    async fn publish_cmaf_video(
        &mut self,
        sample: &VideoSample,
        group: GroupBoundary,
    ) -> Result<()> {
        let Some(cmaf) = &mut self.video_cmaf else {
            return Ok(());
        };
        let Some(object) = cmaf.push(sample, group)? else {
            return Ok(());
        };
        self.broadcast
            .send_object(&cmaf_track_name(&self.tracks.video), object)
            .await?;
        Ok(())
    }

    async fn publish_timeline(
        &mut self,
        group_id: u64,
        presentation_us: u64,
        encoded_at_ms: u64,
    ) -> Result<()> {
        self.timeline
            .record(group_id, presentation_us, encoded_at_ms);
        self.broadcast
            .send_object(
                &timeline_track_name(&self.tracks.video),
                OutgoingObject::plain(GroupBoundary::Next, Bytes::from(self.timeline.document()?)),
            )
            .await?;
        Ok(())
    }

    /// Audio pushed after the video's configuration and before its first
    /// keyframe has no group to belong to and is dropped, as the video before
    /// its first keyframe is. Without video the audio starts the source's
    /// groups, so it writes their media timeline records.
    async fn publish_audio(&mut self, sample: &AudioSample) -> Result<()> {
        self.audio_config
            .as_ref()
            .context("audio sample received before its AudioConfig")?;
        self.announce().await?;
        tracing::debug!(
            target: DELIVERY_LOG_TARGET,
            stage = "ingest",
            namespace = %self.broadcast.namespace().join("/"),
            track = %self.tracks.audio,
            pts_us = sample.pts.micros(),
            bytes = sample.data.len()
        );
        let follows_video = self.audio_follows_video();
        let Some(group) = self.audio_group(sample.pts.micros()) else {
            return Ok(());
        };
        let Some(object) = LocMuxer::new(self.timing.capture_origin(sample.pts))
            .push(&MediaEvent::Audio(sample.clone()))
        else {
            return Ok(());
        };
        let captured_at = object.capture_timestamp();
        let bytes = object.payload.len();
        let written = self
            .broadcast
            .send_object(
                &self.tracks.audio,
                OutgoingObject {
                    group,
                    extension_headers: to_extension_headers(&object.extensions),
                    payload: object.payload,
                },
            )
            .await?;
        if written {
            tracing::debug!(
                target: DELIVERY_LOG_TARGET,
                stage = "publish",
                namespace = %self.broadcast.namespace().join("/"),
                track = %self.tracks.audio,
                group_id = self.audio_group_id.unwrap_or_default(),
                capture_us = captured_at.map_or(0, |at| at.micros()),
                pts_us = sample.pts.micros(),
                bytes
            );
        }
        self.publish_cmaf_audio(sample, group).await?;
        let (GroupBoundary::At(group_id), false, Some(captured_at)) =
            (group, follows_video, captured_at)
        else {
            return Ok(());
        };
        self.publish_timeline(group_id, sample.pts.micros(), captured_at.millis())
            .await
    }

    async fn publish_cmaf_audio(
        &mut self,
        sample: &AudioSample,
        group: GroupBoundary,
    ) -> Result<()> {
        let Some(muxer) = &mut self.audio_cmaf else {
            return Ok(());
        };
        let Some(fragment) = muxer.push(&MediaEvent::Audio(sample.clone()))? else {
            return Ok(());
        };
        self.broadcast
            .send_object(
                &cmaf_track_name(&self.tracks.audio),
                OutgoingObject::plain(group, fragment.data),
            )
            .await?;
        Ok(())
    }

    async fn announce(&mut self) -> Result<()> {
        if !self.announced {
            self.broadcast.announce().await?;
            self.announced = true;
        }
        Ok(())
    }

    /// A rendition numbers a keyframe on its own task, which can be before this
    /// task takes the video's configuration. A group the audio then numbered for
    /// an earlier time would have a later id than the keyframe's group, which
    /// the audio enters next, so the audio numbers none until it has entered
    /// the newest group.
    fn audio_follows_video(&self) -> bool {
        self.video_published || self.timing.alignment.newest() != self.audio_group_id
    }

    /// Without video the audio takes its group ids from the video's alignment,
    /// so they keep increasing when video starts.
    fn audio_group(&mut self, presentation_us: u64) -> Option<GroupBoundary> {
        if !self.audio_follows_video() {
            let due = self.audio_only_group_start_us.is_none_or(|start| {
                presentation_us.saturating_sub(start) >= AUDIO_ONLY_GROUP_DURATION_US
            });
            if !due {
                return Some(GroupBoundary::Within);
            }
            self.audio_only_group_start_us = Some(presentation_us);
            let group_id = self.timing.alignment.keyframe(presentation_us);
            self.audio_group_id = Some(group_id);
            return Some(GroupBoundary::At(group_id));
        }
        self.audio_only_group_start_us = None;
        let group_id = self.timing.alignment.group_covering(presentation_us)?;
        if self.audio_group_id == Some(group_id) {
            return Some(GroupBoundary::Within);
        }
        self.audio_group_id = Some(group_id);
        Some(GroupBoundary::At(group_id))
    }
}

fn wall_clock() -> Timestamp {
    Timestamp::from_micros(executor::system_time().as_micros() as u64)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use mediapack::aac::AudioSpecificConfig;
    use tokio::sync::Notify;

    use super::*;
    use crate::{
        broadcast::{VIDEO_TRACK_NAME, VideoTrackInfo},
        relay_connection::RelayConnection,
        test_support::{FakeRelay, keyframe_at, vp8_config},
    };

    fn publisher() -> TrackPublisher {
        TrackPublisher::new(
            Broadcast::new(RelayConnection::new(None), vec!["ns".to_string()]),
            SourceTracks::default(),
        )
    }

    fn opus_config() -> MediaEvent {
        MediaEvent::AudioConfig(AudioConfig::Opus {
            sample_rate: 48_000,
            channels: 1,
        })
    }

    fn audio_at(pts_us: u64) -> MediaEvent {
        MediaEvent::Audio(AudioSample {
            data: Bytes::from_static(b"opus"),
            pts: Timestamp::from_micros(pts_us),
            audio_level: None,
        })
    }

    fn video_at(dts_us: u64, is_keyframe: bool) -> MediaEvent {
        MediaEvent::Video(VideoSample {
            data: Bytes::from_static(b"frame"),
            is_keyframe,
            pts: Timestamp::from_micros(dts_us),
            dts: Timestamp::from_micros(dts_us),
        })
    }

    fn recorded(publisher: &TrackPublisher) -> Vec<(u64, u64)> {
        let records: Vec<(u64, (u64, u64), u64)> =
            serde_json::from_slice(&publisher.timeline.document().unwrap()).unwrap();
        records
            .into_iter()
            .map(|(presentation_ms, (group_id, _), _)| (presentation_ms, group_id))
            .collect()
    }

    #[tokio::test]
    async fn audio_published_while_video_is_published_asks_for_a_keyframe() {
        // Arrange: a source that keyframes on request has published its video
        let requested = Arc::new(Notify::new());
        let mut publisher = TrackPublisher::new(
            Broadcast::new(RelayConnection::new(None), vec!["ns".to_string()]),
            SourceTracks {
                keyframe_requests: Some(requested.clone()),
                ..SourceTracks::default()
            },
        );
        let video = MediaEvent::VideoConfig(vp8_config());
        publisher.push(&video).await.unwrap();

        // Act
        publisher.push(&opus_config()).await.unwrap();

        // Assert: the audio is sent from the group that keyframe starts
        let asked = tokio::time::timeout(Duration::ZERO, requested.notified()).await;
        assert!(asked.is_ok());
    }

    #[tokio::test]
    async fn without_video_the_audio_records_each_group_it_starts() {
        // Arrange
        let mut publisher = publisher();
        publisher.push(&opus_config()).await.unwrap();

        // Act
        for pts in [0, 20_000, 1_000_000] {
            publisher.push(&audio_at(pts)).await.unwrap();
        }

        // Assert
        let first = publisher.timing.alignment.aligned(0).unwrap();
        assert_eq!(recorded(&publisher), [(0, first), (1_000, first + 1)]);
    }

    #[tokio::test]
    async fn the_audio_continues_the_timeline_once_the_video_is_removed() {
        // Arrange: the audio at 20 ms takes the group of the video keyframe at 0
        let mut publisher = publisher();
        publisher.push(&opus_config()).await.unwrap();
        publisher.video_published = true;
        publisher.push(&keyframe_at(0)).await.unwrap();
        publisher.push(&audio_at(20_000)).await.unwrap();

        // Act
        publisher.remove_track(VIDEO_TRACK_NAME).await.unwrap();
        publisher.push(&audio_at(1_500_000)).await.unwrap();

        // Assert
        let keyframe = publisher.timing.alignment.aligned(0).unwrap();
        assert_eq!(recorded(&publisher), [(0, keyframe), (1_500, keyframe + 1)]);
    }

    #[test]
    fn audio_without_video_starts_a_group_every_second() {
        // Arrange
        let mut publisher = publisher();

        // Act
        let groups: Vec<_> = [0, 20_000, 1_000_000, 1_020_000]
            .map(|pts| publisher.audio_group(pts))
            .into();

        // Assert
        let first = publisher.timing.alignment.aligned(0).unwrap();
        assert_eq!(
            groups,
            [
                Some(GroupBoundary::At(first)),
                Some(GroupBoundary::Within),
                Some(GroupBoundary::At(first + 1)),
                Some(GroupBoundary::Within)
            ]
        );
    }

    #[tokio::test]
    async fn audio_published_again_after_its_removal_starts_a_group_at_once() {
        // Arrange
        let mut publisher = publisher();
        let first = publisher.audio_group(0);
        let audio = publisher.tracks.audio.clone();

        // Act
        publisher.remove_track(&audio).await.unwrap();
        let again = publisher.audio_group(20_000);

        // Assert
        let Some(GroupBoundary::At(first)) = first else {
            panic!("expected the first audio sample to start a group");
        };
        assert_eq!(again, Some(GroupBoundary::At(first + 1)));
    }

    #[test]
    fn audio_waits_for_the_keyframe_a_rendition_numbered_before_the_video_configuration() {
        // Arrange
        let mut publisher = publisher();
        let keyframe = publisher.timing.alignment.keyframe(50_000);

        // Act
        let before_keyframe = publisher.audio_group(40_000);
        let at_keyframe = publisher.audio_group(60_000);
        publisher.video_published = true;
        let after_configuration = publisher.audio_group(80_000);

        // Assert
        assert_eq!(before_keyframe, None);
        assert_eq!(at_keyframe, Some(GroupBoundary::At(keyframe)));
        assert_eq!(after_configuration, Some(GroupBoundary::Within));
    }

    #[tokio::test]
    async fn the_audio_leaves_the_record_of_a_group_a_rendition_started_to_the_video() {
        // Arrange
        let mut publisher = publisher();
        publisher.timing.alignment.keyframe(50_000);
        publisher.push(&opus_config()).await.unwrap();

        // Act
        publisher.push(&audio_at(60_000)).await.unwrap();

        // Assert
        assert_eq!(recorded(&publisher), []);
    }

    #[tokio::test]
    async fn audio_follows_the_video_keyframes_until_the_video_is_removed() {
        // Arrange: video is published and has a keyframe at 0
        let mut publisher = publisher();
        publisher.video_published = true;
        let keyframe = publisher.timing.alignment.keyframe(0);

        // Act
        let with_video = publisher.audio_group(1_500_000);
        publisher.remove_track(VIDEO_TRACK_NAME).await.unwrap();
        let without_video = publisher.audio_group(1_520_000);

        // Assert
        assert_eq!(with_video, Some(GroupBoundary::At(keyframe)));
        assert_eq!(without_video, Some(GroupBoundary::At(keyframe + 1)));
    }

    #[tokio::test]
    async fn the_cmaf_keyframe_whose_decode_time_repeats_takes_the_loc_keyframe_group() {
        // Arrange: the CMAF muxer moves a keyframe that repeats the previous decode time one tick later
        let mut publisher = TrackPublisher::new(
            Broadcast::new(RelayConnection::new(None), vec!["ns".to_string()]),
            SourceTracks {
                cmaf: true,
                ..SourceTracks::default()
            },
        );
        let video = MediaEvent::VideoConfig(vp8_config());
        publisher.push(&video).await.unwrap();
        publisher.push(&opus_config()).await.unwrap();

        // Act
        for (dts_us, is_keyframe) in [(0, true), (40_000, false), (40_000, true), (80_000, false)] {
            publisher
                .push(&video_at(dts_us, is_keyframe))
                .await
                .unwrap();
        }
        publisher.push(&audio_at(60_000)).await.unwrap();

        // Assert: no group is numbered beyond the LOC keyframe's, and the audio enters it
        let keyframe = publisher.timing.alignment.aligned(40_000);
        assert_eq!(publisher.timing.alignment.newest(), keyframe);
        assert_eq!(publisher.audio_group_id, keyframe);
    }

    mod properties {
        use std::collections::BTreeSet;

        use proptest::prelude::*;

        use super::*;
        use crate::{
            cmaf_video::CmafVideo,
            group_alignment::{REMEMBERED_KEYFRAMES, StartedGroups},
        };

        /// One VIDEO_TIMESCALE tick of the fMP4 muxer is 11.1 µs, so gaps of
        /// 0 to 3 ticks reach the samples it moves one tick later.
        fn gap_us(frame_us: u64) -> impl Strategy<Value = u64> {
            prop_oneof![Just(0), 1..=34u64, Just(frame_us)]
        }

        #[derive(Debug, Clone)]
        enum Step {
            /// The source's next frame, and whether the rendition keyframes on it.
            Source {
                gap_us: u64,
                is_keyframe: bool,
                rendition_keyframe: bool,
            },
            /// The rendition takes the next frame the source has taken, if any.
            Rendition,
            Audio {
                gap_us: u64,
            },
        }

        fn step() -> impl Strategy<Value = Step> {
            prop_oneof![
                (gap_us(33_333), any::<bool>(), any::<bool>()).prop_map(
                    |(gap_us, is_keyframe, rendition_keyframe)| Step::Source {
                        gap_us,
                        is_keyframe,
                        rendition_keyframe,
                    }
                ),
                Just(Step::Rendition),
                gap_us(21_333).prop_map(|gap_us| Step::Audio { gap_us }),
            ]
        }

        fn sample(dts_us: u64, is_keyframe: bool) -> VideoSample {
            VideoSample {
                data: Bytes::from_static(b"frame"),
                is_keyframe,
                pts: Timestamp::from_micros(dts_us),
                dts: Timestamp::from_micros(dts_us),
            }
        }

        fn group_ids(boundaries: &[GroupBoundary]) -> Vec<u64> {
            boundaries
                .iter()
                .filter_map(|boundary| match boundary {
                    GroupBoundary::At(group_id) => Some(*group_id),
                    _ => None,
                })
                .collect()
        }

        #[derive(Default)]
        struct Published {
            video: Vec<GroupBoundary>,
            video_cmaf: Vec<GroupBoundary>,
            rendition: Vec<GroupBoundary>,
            rendition_cmaf: Vec<GroupBoundary>,
            audio: Vec<GroupBoundary>,
            /// The rendition keyframes on a source keyframe still remembered
            /// that neither took its group id nor followed a rendition frame of
            /// the same time that did, with the id they missed.
            rendition_misses: Vec<(u64, GroupBoundary)>,
        }

        /// Numbers the groups the way `publish_video`, a `FollowSource`
        /// `RenditionTrack` and `publish_audio` do.
        fn publish(steps: &[Step]) -> Published {
            let mut publisher = publisher();
            publisher.video_published = true;
            let alignment = publisher.timing.alignment.clone();
            let mut video_cmaf = CmafVideo::new(vp8_config());
            let mut rendition_cmaf = CmafVideo::new(vp8_config());
            let mut published = Published::default();
            let mut source_frames = Vec::new();
            let mut source_keyframes = 0;
            let mut rendition_taken = 0;
            let (mut video_us, mut audio_us) = (0, 0);
            let (mut video_groups, mut rendition_groups) =
                (StartedGroups::default(), StartedGroups::default());
            for step in steps {
                match *step {
                    Step::Source {
                        gap_us,
                        is_keyframe,
                        rendition_keyframe,
                    } => {
                        video_us += gap_us;
                        let frame = sample(video_us, is_keyframe);
                        let group = video_groups
                            .boundary(frame.is_keyframe.then(|| alignment.keyframe(video_us)));
                        published.video.push(group);
                        source_keyframes += usize::from(is_keyframe);
                        if let Some(object) = video_cmaf.push(&frame, group).unwrap() {
                            published.video_cmaf.push(object.group);
                        }
                        source_frames.push((
                            sample(video_us, rendition_keyframe),
                            group,
                            source_keyframes,
                        ));
                    }
                    Step::Rendition => {
                        let Some((frame, source_group, keyframe_number)) =
                            source_frames.get(rendition_taken)
                        else {
                            continue;
                        };
                        rendition_taken += 1;
                        let group = rendition_groups.boundary(
                            frame
                                .is_keyframe
                                .then(|| alignment.aligned(frame.pts.micros()))
                                .flatten(),
                        );
                        let already_started =
                            |id: &u64| published.rendition.contains(&GroupBoundary::At(*id));
                        if let GroupBoundary::At(source_id) = source_group
                            && frame.is_keyframe
                            && source_keyframes - keyframe_number < REMEMBERED_KEYFRAMES
                            && group != *source_group
                            && !already_started(source_id)
                        {
                            published.rendition_misses.push((*source_id, group));
                        }
                        published.rendition.push(group);
                        if let Some(object) = rendition_cmaf.push(frame, group).unwrap() {
                            published.rendition_cmaf.push(object.group);
                        }
                    }
                    Step::Audio { gap_us } => {
                        audio_us += gap_us;
                        if let Some(group) = publisher.audio_group(audio_us) {
                            published.audio.push(group);
                        }
                    }
                }
            }
            published
        }

        proptest! {
            #[test]
            fn every_track_takes_source_video_group_ids_in_increasing_order(
                steps in prop::collection::vec(step(), 0..600),
            ) {
                // Act
                let published = publish(&steps);

                // Assert: `Origin` rejects a group id that does not increase
                let source_ids: BTreeSet<u64> = group_ids(&published.video).into_iter().collect();
                for (track, boundaries) in [
                    ("video", &published.video),
                    ("video_cmaf", &published.video_cmaf),
                    ("rendition", &published.rendition),
                    ("rendition_cmaf", &published.rendition_cmaf),
                    ("audio", &published.audio),
                ] {
                    let ids = group_ids(boundaries);
                    prop_assert!(ids.windows(2).all(|pair| pair[0] < pair[1]), "{track}: {ids:?}");
                    prop_assert!(ids.iter().all(|id| source_ids.contains(id)), "{track}: {ids:?}");
                }
                // Assert: a rendition keyframe takes the id of the source keyframe it follows
                prop_assert_eq!(&published.rendition_misses, &[]);
                // Assert: each fragment starts the group its LOC object started (cmsf-01 §3.2)
                prop_assert_eq!(
                    &published.video_cmaf[..],
                    &published.video[..published.video_cmaf.len()]
                );
                prop_assert_eq!(
                    &published.rendition_cmaf[..],
                    &published.rendition[..published.rendition_cmaf.len()]
                );
            }
        }
    }
    fn relay_publisher(relay: &FakeRelay, tracks: SourceTracks) -> TrackPublisher {
        TrackPublisher::new(
            Broadcast::new(relay.connection(), vec!["live".to_string()]),
            tracks,
        )
    }

    #[tokio::test]
    async fn the_video_is_listed_with_its_source_s_tracks() {
        // Arrange: the source is a camera profile, as `ingest-gateway pull` names it
        let mut relay = FakeRelay::spawn("publisher-catalog");
        let broadcast = Broadcast::new(relay.connection(), vec!["live".to_string()]);
        broadcast
            .update_video_catalog(
                "video/profile_2",
                VideoTrackInfo {
                    switching_set: "profile".to_string(),
                    source: "video/profile_2".to_string(),
                    cmaf: false,
                    ..VideoTrackInfo::from_config(&vp8_config(), "Profile 2".to_string()).unwrap()
                },
            )
            .await
            .unwrap();
        let mut publisher = TrackPublisher::new(
            broadcast,
            SourceTracks {
                video: "video/profile_1".to_string(),
                switching_set: "profile".to_string(),
                cmaf: false,
                video_bitrate: Some(300_000),
                video_framerate: Some(15),
                ..SourceTracks::default()
            },
        );

        // Act
        publisher
            .push(&MediaEvent::VideoConfig(vp8_config()))
            .await
            .unwrap();
        let catalog = relay.newest_catalog().await;

        // Assert
        let tracks = catalog.tracks;
        let track = |name: &str| tracks.iter().find(|track| track.name == name);
        let video = track("video/profile_1").expect("the video must be listed");
        let other = track("video/profile_2").unwrap();
        assert_eq!(video.bitrate, Some(300_000));
        assert_eq!(video.framerate, Some(15.0));
        assert!(video.alt_group.is_some() && video.alt_group == other.alt_group);
        assert!(track("video/profile_1_cmaf").is_none());
        assert!(track("video/profile_1_timeline").is_some());
        assert!(track("video_timeline").is_none());
    }

    #[tokio::test]
    async fn a_subscriber_of_the_cmaf_video_receives_its_fragments() {
        // Arrange
        let mut relay = FakeRelay::spawn("publisher-cmaf-video");
        let mut publisher = relay_publisher(&relay, SourceTracks::default());
        publisher
            .push(&MediaEvent::VideoConfig(vp8_config()))
            .await
            .unwrap();
        let subscription = relay.subscribe("live", "video_cmaf").await;

        // Act: a fragment is muxed once the next sample gives its duration
        publisher.push(&keyframe_at(40_000)).await.unwrap();
        publisher.push(&video_at(80_000, false)).await.unwrap();

        // Assert
        let fragment = relay.first_object(&subscription).await;
        assert!(fragment.payload.windows(4).any(|window| window == b"moof"));
    }

    #[tokio::test]
    async fn a_subscriber_of_the_cmaf_audio_receives_its_fragments() {
        // Arrange
        let mut relay = FakeRelay::spawn("publisher-cmaf-audio");
        let mut publisher = relay_publisher(&relay, SourceTracks::default());
        publisher
            .push(&MediaEvent::AudioConfig(AudioConfig::Aac(
                AudioSpecificConfig::new(2, 48_000, 1),
            )))
            .await
            .unwrap();
        let subscription = relay.subscribe("live", "audio_cmaf").await;

        // Act
        publisher.push(&audio_at(0)).await.unwrap();

        // Assert
        let fragment = relay.first_object(&subscription).await;
        assert!(fragment.payload.windows(4).any(|window| window == b"moof"));
    }
}
