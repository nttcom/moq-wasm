use anyhow::{Context, Result};
use mediapack::{
    MediaEvent,
    loc::{VideoCodec, to_extension_headers},
    mp4::Fmp4TrackMuxer,
};
use moqt::publish::{GroupBoundary, OutgoingObject};

use crate::{
    broadcast::{Broadcast, VideoTrackInfo, cmaf_track_name},
    group_alignment::GroupAlignment,
    track_publisher::SharedTiming,
    writer::BroadcastWriter,
};

pub struct RenditionConfig {
    pub track: String,
    pub label: String,
    pub bitrate: Option<u64>,
    pub cmaf: bool,
}

/// Writes one rendition of a writer's source video into its own track, in the
/// group ids the source took at the same presentation times
/// (draft-ietf-moq-cmsf-01 §3.2). Frames before its first keyframe aligned
/// with a source keyframe are dropped.
pub struct RenditionTrack {
    broadcast: Broadcast,
    timing: SharedTiming,
    config: RenditionConfig,
    video_codec: VideoCodec,
    cmaf: Option<Fmp4TrackMuxer>,
}

impl RenditionTrack {
    pub fn new(writer: &BroadcastWriter, config: RenditionConfig) -> Self {
        Self {
            broadcast: writer.broadcast().clone(),
            timing: writer.shared_timing(),
            config,
            video_codec: VideoCodec::H264,
            cmaf: None,
        }
    }

    /// Ignores events other than video. Fails on a sample pushed before the
    /// writer took its first source sample, which seeds the LOC capture origin.
    pub async fn push(&mut self, event: MediaEvent) -> Result<()> {
        match event {
            MediaEvent::VideoConfig(config) => {
                self.video_codec = VideoCodec::from(&config);
                self.cmaf = self
                    .config
                    .cmaf
                    .then(|| Fmp4TrackMuxer::video(config.clone()));
                let info = VideoTrackInfo {
                    bitrate: self.config.bitrate,
                    cmaf: self.config.cmaf,
                    ..VideoTrackInfo::from_config(&config, self.config.label.clone())?
                };
                self.broadcast
                    .update_video_catalog(&self.config.track, info)
                    .await
            }
            MediaEvent::Video(sample) => {
                let track = &self.config.track;
                tracing::trace!(%track, pts = sample.pts.micros(), is_keyframe = sample.is_keyframe, "rendition sample received");
                let alignment = &self.timing.alignment;
                if sample.is_keyframe && alignment.aligned(sample.pts.micros()).is_none() {
                    tracing::warn!(
                        %track,
                        pts_us = sample.pts.micros(),
                        nearest_source_keyframe_us = ?alignment.nearest_keyframe_us(sample.pts.micros()),
                        "rendition keyframe has no aligned source keyframe; its frames are dropped until one aligns"
                    );
                }
                let fragment = match &mut self.cmaf {
                    Some(muxer) => muxer.push(&MediaEvent::Video(sample.clone()))?,
                    None => None,
                };
                let group = aligned_boundary(alignment, sample.is_keyframe, sample.pts.micros());
                let muxer = self
                    .timing
                    .loc
                    .get()
                    .context("rendition sample before the source seeded the LOC capture origin")?
                    .with_video_codec(self.video_codec);
                if let Some(object) = muxer.push(&MediaEvent::Video(sample)) {
                    self.broadcast
                        .send_object(
                            track,
                            OutgoingObject {
                                group,
                                extension_headers: to_extension_headers(&object.extensions),
                                payload: object.payload,
                            },
                        )
                        .await?;
                }
                let Some(fragment) = fragment else {
                    return Ok(());
                };
                let group = aligned_boundary(
                    &self.timing.alignment,
                    fragment.is_keyframe,
                    fragment.presentation_time.micros(),
                );
                self.broadcast
                    .send_object(
                        &cmaf_track_name(track),
                        OutgoingObject::plain(group, fragment.data),
                    )
                    .await?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

fn aligned_boundary(
    alignment: &GroupAlignment,
    is_keyframe: bool,
    presentation_us: u64,
) -> GroupBoundary {
    match (is_keyframe, alignment.aligned(presentation_us)) {
        (true, Some(group_id)) => GroupBoundary::At(group_id),
        _ => GroupBoundary::Within,
    }
}
