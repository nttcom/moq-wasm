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
    group_alignment::GroupAlignment,
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
    pub(crate) loc: Arc<OnceLock<LocMuxer>>,
    pub(crate) alignment: Arc<GroupAlignment>,
}

impl SharedTiming {
    /// The first call takes the wall clock as the capture time of
    /// `presentation_time`; later calls ignore it.
    pub(crate) fn loc_muxer(&self, presentation_time: Timestamp) -> &LocMuxer {
        self.loc
            .get_or_init(|| LocMuxer::new(wall_clock().saturating_sub(presentation_time)))
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
    video_cmaf: Option<Fmp4TrackMuxer>,
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
            timing: SharedTiming {
                loc: Arc::new(OnceLock::new()),
                alignment: Arc::new(GroupAlignment::new(wall_clock().micros())),
            },
            video_codec: VideoCodec::H264,
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
                self.video_cmaf = self
                    .tracks
                    .cmaf
                    .then(|| Fmp4TrackMuxer::video(config.clone()));
                let info = VideoTrackInfo {
                    cmaf: self.tracks.cmaf,
                    switching_set: self.tracks.switching_set.clone(),
                    source: self.tracks.video.clone(),
                    bitrate: self.tracks.video_bitrate,
                    framerate: self.tracks.video_framerate,
                    ..VideoTrackInfo::from_config(config, self.tracks.video_label.clone())?
                };
                if let Some(requested) = &self.tracks.keyframe_requests {
                    self.broadcast
                        .accept_keyframe_requests(&self.tracks.video, requested.clone())
                        .await?;
                }
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
        let Some(object) = self
            .timing
            .loc_muxer(sample.pts)
            .with_video_codec(self.video_codec)
            .push(&MediaEvent::Video(sample.clone()))
        else {
            return Ok(());
        };
        let captured_at = object.capture_timestamp();
        let bytes = object.payload.len();
        let keyframe_group = sample
            .is_keyframe
            .then(|| self.timing.alignment.keyframe(sample.pts.micros()));
        let written = self
            .broadcast
            .send_object(
                &self.tracks.video,
                OutgoingObject {
                    group: keyframe_group.map_or(GroupBoundary::Within, GroupBoundary::At),
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
        self.publish_cmaf_video(sample).await?;
        let (Some(group_id), Some(captured_at)) = (keyframe_group, captured_at) else {
            return Ok(());
        };
        self.publish_timeline(group_id, sample.pts.micros(), captured_at.millis())
            .await
    }

    async fn publish_cmaf_video(&mut self, sample: &VideoSample) -> Result<()> {
        let Some(muxer) = &mut self.video_cmaf else {
            return Ok(());
        };
        let Some(fragment) = muxer.push(&MediaEvent::Video(sample.clone()))? else {
            return Ok(());
        };
        let group = if fragment.is_keyframe {
            GroupBoundary::At(
                self.timing
                    .alignment
                    .keyframe(fragment.presentation_time.micros()),
            )
        } else {
            GroupBoundary::Within
        };
        self.broadcast
            .send_object(
                &cmaf_track_name(&self.tracks.video),
                OutgoingObject::plain(group, fragment.data),
            )
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
        let Some(group) = self.audio_group(sample.pts.micros()) else {
            return Ok(());
        };
        let Some(object) = self
            .timing
            .loc_muxer(sample.pts)
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
            (group, self.video_published, captured_at)
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

    /// Without video the audio takes its group ids from the video's alignment,
    /// so they keep increasing when video starts.
    fn audio_group(&mut self, presentation_us: u64) -> Option<GroupBoundary> {
        if !self.video_published {
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

    use mediapack::{VideoConfig, vp8::Vp8Config};
    use tokio::sync::Notify;

    use super::*;
    use crate::{broadcast::VIDEO_TRACK_NAME, relay_connection::RelayConnection};

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

    fn keyframe_at(pts_us: u64) -> MediaEvent {
        MediaEvent::Video(VideoSample {
            data: Bytes::from_static(b"nal"),
            is_keyframe: true,
            pts: Timestamp::from_micros(pts_us),
            dts: Timestamp::from_micros(pts_us),
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
        let video = MediaEvent::VideoConfig(VideoConfig::Vp8(Vp8Config {
            width: 64,
            height: 48,
        }));
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
}
