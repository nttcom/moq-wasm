use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use bytes::Bytes;
use mediapack::{
    AudioConfig, AudioSample, MediaEvent, Timestamp, VideoSample,
    loc::{Muxer as LocMuxer, to_extension_headers},
    mp4::Fmp4TrackMuxer,
};
use moqt::{
    executor,
    publish::{GroupBoundary, OutgoingObject},
};

use crate::{
    broadcast::{
        AudioTrackInfo, Broadcast, SourceTracks, TIMELINE_TRACK_NAME, VIDEO_TRACK_NAME,
        VideoTrackInfo, cmaf_track_name,
    },
    group_alignment::GroupAlignment,
    media_timeline::MediaTimeline,
};

/// One DEBUG line per sample as it enters (`ingest`) and as it is sent to the
/// relay (`publish`, with the group id and LOC capture timestamp a viewer
/// sees), so that what a viewer received can be checked against
/// what was sent: `RUST_LOG=broadcast::delivery=debug`.
const DELIVERY_LOG_TARGET: &str = "broadcast::delivery";

/// The clocks every track of one switching set stamps from, so renditions of
/// the source video carry the same capture timestamp and group id for the same
/// presentation time (draft-ietf-moq-cmsf-01 §3.2).
#[derive(Clone)]
pub struct SharedTiming {
    pub loc: Arc<OnceLock<LocMuxer>>,
    pub alignment: Arc<GroupAlignment>,
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
    /// The video keyframe group the audio is being written into, so that the
    /// audio tracks start a group exactly when the video tracks do.
    audio_group_id: Option<u64>,
    audio_config: Option<AudioConfig>,
    timeline: MediaTimeline,
    timing: SharedTiming,
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
            audio_config: None,
            timeline: MediaTimeline::default(),
            timing: SharedTiming {
                loc: Arc::new(OnceLock::new()),
                alignment: Arc::new(GroupAlignment::new(wall_clock().micros())),
            },
            video_cmaf: None,
            audio_cmaf: None,
        }
    }

    pub(crate) fn shared_timing(&self) -> SharedTiming {
        self.timing.clone()
    }

    pub(crate) async fn push(&mut self, event: &MediaEvent) -> Result<()> {
        match event {
            MediaEvent::Streams(_) => Ok(()),
            MediaEvent::VideoConfig(config) => {
                self.video_cmaf = Some(Fmp4TrackMuxer::video(config.clone()));
                let info = VideoTrackInfo::from_record(config, self.tracks.video_label.clone())?;
                self.broadcast
                    .update_video_catalog(&self.tracks.video, info)
                    .await
            }
            MediaEvent::AudioConfig(config) => {
                self.audio_cmaf = match config {
                    AudioConfig::Aac(_) => Some(Fmp4TrackMuxer::audio(config.clone())),
                    _ => None,
                };
                let info = AudioTrackInfo {
                    label: self.tracks.audio_label.clone(),
                    config: config.clone(),
                };
                self.broadcast
                    .update_audio_catalog(&self.tracks.audio, info)
                    .await?;
                self.audio_config = Some(config.clone());
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
        if self.tracks.video != VIDEO_TRACK_NAME {
            return Ok(());
        }
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
                TIMELINE_TRACK_NAME,
                OutgoingObject::plain(GroupBoundary::Next, Bytes::from(self.timeline.document()?)),
            )
            .await?;
        Ok(())
    }

    /// Audio before the first video keyframe has no group to belong to and is
    /// dropped, as the video before its first keyframe is.
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
        let capture_us = object.capture_timestamp().map_or(0, |at| at.micros());
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
                capture_us,
                pts_us = sample.pts.micros(),
                bytes
            );
        }
        self.publish_cmaf_audio(sample, group).await
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

    fn audio_group(&mut self, presentation_us: u64) -> Option<GroupBoundary> {
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
