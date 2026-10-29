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

/// How a rendition finds the group id of its keyframes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenditionKeyframes {
    /// A keyframe takes the group id the source took at its presentation
    /// time, and one the source has not taken stays in the current group: for
    /// a transcoder asked to keyframe on the source's keyframes.
    FollowSource,
    /// A keyframe takes the group id for its presentation time, whether the
    /// source or the rendition reaches that time first, and the rendition may
    /// seed the LOC capture origin: for renditions encoded from the source's
    /// frames and keyframed on the same frames.
    SameFrames,
}

pub struct RenditionConfig {
    pub track: String,
    pub label: String,
    pub bitrate: Option<u64>,
    pub cmaf: bool,
    pub keyframes: RenditionKeyframes,
}

/// Writes one rendition of a writer's source video into its own track, in the
/// group ids of the source's keyframes at the same presentation times
/// (draft-ietf-moq-cmsf-01 §3.2).
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

    /// Ignores events other than video. Under `FollowSource`, fails on a
    /// sample pushed before the writer took its first source sample, which
    /// seeds the LOC capture origin.
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
                if self.config.keyframes == RenditionKeyframes::FollowSource
                    && sample.is_keyframe
                    && alignment.aligned(sample.pts.micros()).is_none()
                {
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
                let group = self.boundary(sample.is_keyframe, sample.pts.micros());
                let muxer = match self.config.keyframes {
                    RenditionKeyframes::FollowSource => *self.timing.loc.get().context(
                        "rendition sample before the source seeded the LOC capture origin",
                    )?,
                    RenditionKeyframes::SameFrames => *self.timing.loc_muxer(sample.pts),
                }
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
                let group =
                    self.boundary(fragment.is_keyframe, fragment.presentation_time.micros());
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

impl RenditionTrack {
    fn boundary(&self, is_keyframe: bool, presentation_us: u64) -> GroupBoundary {
        if !is_keyframe {
            return GroupBoundary::Within;
        }
        let alignment: &GroupAlignment = &self.timing.alignment;
        let group_id = match self.config.keyframes {
            RenditionKeyframes::FollowSource => alignment.aligned(presentation_us),
            RenditionKeyframes::SameFrames => Some(alignment.keyframe(presentation_us)),
        };
        group_id.map_or(GroupBoundary::Within, GroupBoundary::At)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{broadcast::SourceTracks, relay_connection::RelayConnection};

    fn rendition(writer: &BroadcastWriter, keyframes: RenditionKeyframes) -> RenditionTrack {
        RenditionTrack::new(
            writer,
            RenditionConfig {
                track: "video_low".to_string(),
                label: "Low".to_string(),
                bitrate: None,
                cmaf: false,
                keyframes,
            },
        )
    }

    #[tokio::test]
    async fn a_same_frames_keyframe_takes_the_id_the_source_takes_whichever_comes_first() {
        // Arrange
        let broadcast = Broadcast::new(RelayConnection::new(None), vec!["ns".to_string()]);
        let writer = BroadcastWriter::run(broadcast, SourceTracks::default());
        let same_frames = rendition(&writer, RenditionKeyframes::SameFrames);
        let follow_source = rendition(&writer, RenditionKeyframes::FollowSource);

        // Act
        let before_source = follow_source.boundary(true, 40_000);
        let rendition_first = same_frames.boundary(true, 40_000);
        let source = writer.shared_timing().alignment.keyframe(40_000);

        // Assert
        assert_eq!(before_source, GroupBoundary::Within);
        assert_eq!(rendition_first, GroupBoundary::At(source));
        assert_eq!(
            follow_source.boundary(true, 40_000),
            GroupBoundary::At(source)
        );
        assert_eq!(same_frames.boundary(false, 80_000), GroupBoundary::Within);
    }
}
