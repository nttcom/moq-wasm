use std::sync::Arc;

use anyhow::{Context, Result};
use mediapack::{
    MediaEvent,
    loc::{Muxer as LocMuxer, VideoCodec, to_extension_headers},
};
use moqt::publish::{GroupBoundary, OutgoingObject, TrackDemand};
use tokio::sync::{Notify, mpsc};

use crate::{
    broadcast::{Broadcast, VideoTrackInfo, cmaf_track_name},
    cmaf_video::CmafVideo,
    group_alignment::{GroupAlignment, StartedGroups},
    track_publisher::SharedTiming,
    writer::BroadcastWriter,
};

/// How a rendition finds the group id of its keyframes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenditionKeyframes {
    /// A keyframe takes the group id the source took at its presentation
    /// time, and one the source has not taken stays in the current group: for
    /// a transcoder asked to keyframe on the source's keyframes and fed each
    /// sample only after it was passed to `BroadcastWriter::push`.
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
    /// Frames per second, when the rendition's encoder caps them.
    pub framerate: Option<u32>,
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
    /// The source's, whose alternate the rendition is.
    switching_set: String,
    /// `SourceTracks::video` of the source, whose media timeline lists the
    /// rendition's groups.
    source: String,
    video_codec: VideoCodec,
    groups: StartedGroups,
    cmaf: Option<CmafVideo>,
    /// The source's `SourceTracks::keyframe_requests`: the rendition keyframes
    /// on the same frames as the source.
    keyframe_requests: Option<Arc<Notify>>,
    demand_sender: Option<mpsc::UnboundedSender<TrackDemand>>,
}

impl RenditionTrack {
    pub fn new(writer: &BroadcastWriter, config: RenditionConfig) -> Self {
        Self {
            broadcast: writer.broadcast().clone(),
            timing: writer.shared_timing(),
            switching_set: writer.source_tracks().switching_set.clone(),
            source: writer.source_tracks().video.clone(),
            config,
            video_codec: VideoCodec::H264,
            groups: StartedGroups::default(),
            cmaf: None,
            keyframe_requests: writer.source_tracks().keyframe_requests.clone(),
            demand_sender: writer.source_tracks().demand_sender.clone(),
        }
    }

    /// Ignores events other than video. Under `FollowSource`, fails on a
    /// sample pushed before the writer took its first source sample, which
    /// seeds the LOC capture origin.
    pub async fn push(&mut self, event: MediaEvent) -> Result<()> {
        match event {
            MediaEvent::VideoConfig(config) => {
                self.video_codec = VideoCodec::from(&config);
                self.cmaf = self.config.cmaf.then(|| CmafVideo::new(config.clone()));
                let info = VideoTrackInfo {
                    bitrate: self.config.bitrate,
                    framerate: self.config.framerate,
                    switching_set: self.switching_set.clone(),
                    source: self.source.clone(),
                    cmaf: self.config.cmaf,
                    ..VideoTrackInfo::from_config(&config, self.config.label.clone())?
                };
                self.broadcast
                    .prepare_encoded_video(
                        &self.config.track,
                        self.config.cmaf,
                        self.keyframe_requests.as_ref(),
                        self.demand_sender.as_ref(),
                    )
                    .await?;
                self.broadcast
                    .update_video_catalog(&self.config.track, info)
                    .await
            }
            MediaEvent::Video(sample) => {
                let track = &self.config.track.clone();
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
                let group = self.boundary(sample.is_keyframe, sample.pts.micros());
                let capture_origin = match self.config.keyframes {
                    RenditionKeyframes::FollowSource => {
                        self.timing.seeded_capture_origin().context(
                            "rendition sample before the source seeded the LOC capture origin",
                        )?
                    }
                    RenditionKeyframes::SameFrames => self.timing.capture_origin(sample.pts),
                };
                let cmaf_object = match &mut self.cmaf {
                    Some(cmaf) => cmaf.push(&sample, group, capture_origin)?,
                    None => None,
                };
                let muxer = LocMuxer::new(capture_origin).with_video_codec(self.video_codec);
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
                let Some(object) = cmaf_object else {
                    return Ok(());
                };
                self.broadcast
                    .send_object(&cmaf_track_name(track), object)
                    .await?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

impl RenditionTrack {
    fn boundary(&mut self, is_keyframe: bool, presentation_us: u64) -> GroupBoundary {
        if !is_keyframe {
            return GroupBoundary::Within;
        }
        let alignment: &GroupAlignment = &self.timing.alignment;
        let group_id = match self.config.keyframes {
            RenditionKeyframes::FollowSource => alignment.aligned(presentation_us),
            RenditionKeyframes::SameFrames => Some(alignment.keyframe(presentation_us)),
        };
        self.groups.boundary(group_id)
    }
}

#[cfg(test)]
mod tests {
    use mediapack::{Timestamp, mp4::Fmp4TrackMuxer};

    use super::*;
    use crate::{
        broadcast::SourceTracks,
        broadcast::{VIDEO_TRACK_NAME, VideoTrackInfo},
        relay_connection::RelayConnection,
        test_support::{
            FakeRelay, TEST_TIMEOUT, VIDEO_TICK_US, decode_times, keyframe_at, vp8_config,
        },
    };

    fn rendition_config(keyframes: RenditionKeyframes) -> RenditionConfig {
        RenditionConfig {
            track: "video_low".to_string(),
            label: "Low".to_string(),
            bitrate: None,
            framerate: None,
            cmaf: false,
            keyframes,
        }
    }

    fn rendition(writer: &BroadcastWriter, keyframes: RenditionKeyframes) -> RenditionTrack {
        RenditionTrack::new(writer, rendition_config(keyframes))
    }

    #[tokio::test]
    async fn a_same_frames_keyframe_takes_the_id_the_source_takes_whichever_comes_first() {
        // Arrange
        let broadcast = Broadcast::new(RelayConnection::new(None), vec!["ns".to_string()]);
        let writer = BroadcastWriter::run(broadcast, SourceTracks::default());
        let mut same_frames = rendition(&writer, RenditionKeyframes::SameFrames);
        let mut follow_source = rendition(&writer, RenditionKeyframes::FollowSource);

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

    #[tokio::test]
    async fn a_follow_source_keyframe_takes_the_id_of_a_source_keyframe_still_queued() {
        // Arrange: nothing awaits, so the publish task never takes the keyframe off the queue
        let broadcast = Broadcast::new(RelayConnection::new(None), vec!["ns".to_string()]);
        let mut writer = BroadcastWriter::run(broadcast, SourceTracks::default());
        let mut rendition = rendition(&writer, RenditionKeyframes::FollowSource);
        writer.push(keyframe_at(40_000)).unwrap();

        // Act
        let boundary = rendition.boundary(true, 40_000);

        // Assert
        let source = writer.shared_timing().alignment.aligned(40_000);
        assert_eq!(boundary, GroupBoundary::At(source.unwrap()));
    }

    #[tokio::test]
    async fn a_keyframe_that_repeats_the_previous_keyframes_time_continues_its_group() {
        // Arrange
        let broadcast = Broadcast::new(RelayConnection::new(None), vec!["ns".to_string()]);
        let writer = BroadcastWriter::run(broadcast, SourceTracks::default());
        let mut rendition = rendition(&writer, RenditionKeyframes::SameFrames);
        let first = rendition.boundary(true, 40_000);

        // Act
        let repeated = rendition.boundary(true, 40_000);

        // Assert
        assert!(matches!(first, GroupBoundary::At(_)), "{first:?}");
        assert_eq!(repeated, GroupBoundary::Within);
    }
    #[tokio::test]
    async fn a_rendition_reports_its_demand_where_its_source_does() {
        // Arrange
        let relay = FakeRelay::spawn("rendition-demand");
        let broadcast = Broadcast::new(relay.connection(), vec!["live".to_string()]);
        let (demand_sender, mut demand_receiver) = mpsc::unbounded_channel();
        let writer = BroadcastWriter::run(
            broadcast,
            SourceTracks {
                demand_sender: Some(demand_sender),
                ..SourceTracks::default()
            },
        );
        let mut rendition = rendition(&writer, RenditionKeyframes::SameFrames);

        // Act
        rendition
            .push(MediaEvent::VideoConfig(vp8_config()))
            .await
            .unwrap();
        let demand = tokio::time::timeout(TEST_TIMEOUT, demand_receiver.recv()).await;

        // Assert
        let demand = demand
            .expect("the rendition must report its demand")
            .unwrap();
        assert_eq!(demand.track_name, "video_low");
    }
    #[tokio::test]
    async fn a_rendition_is_listed_with_its_config_in_its_source_s_switching_set() {
        // Arrange: the source is a camera profile, as `ingest-gateway pull` names it
        let mut relay = FakeRelay::spawn("rendition-catalog");
        let broadcast = Broadcast::new(relay.connection(), vec!["live".to_string()]);
        let source = SourceTracks {
            video: "video/profile_1".to_string(),
            switching_set: "profile_1".to_string(),
            ..SourceTracks::default()
        };
        broadcast
            .update_video_catalog(
                &source.video,
                VideoTrackInfo {
                    switching_set: source.switching_set.clone(),
                    source: source.video.clone(),
                    cmaf: false,
                    ..VideoTrackInfo::from_config(&vp8_config(), "Profile 1".to_string()).unwrap()
                },
            )
            .await
            .unwrap();
        let writer = BroadcastWriter::run(broadcast, source);
        let mut rendition = RenditionTrack::new(
            &writer,
            RenditionConfig {
                track: "video_low".to_string(),
                label: "Low".to_string(),
                bitrate: Some(300_000),
                framerate: Some(15),
                cmaf: false,
                keyframes: RenditionKeyframes::SameFrames,
            },
        );

        // Act
        rendition
            .push(MediaEvent::VideoConfig(vp8_config()))
            .await
            .unwrap();
        let catalog = relay.newest_catalog().await;

        // Assert
        let tracks = catalog.tracks;
        let track = |name: &str| tracks.iter().find(|track| track.name == name);
        let low = track("video_low").expect("the rendition must be listed");
        let profile = track("video/profile_1").unwrap();
        assert_eq!(low.bitrate, Some(300_000));
        assert_eq!(low.framerate, Some(15.0));
        assert!(low.alt_group.is_some() && low.alt_group == profile.alt_group);
        assert!(track("video_low_cmaf").is_none());
        assert!(track("video/profile_1_timeline").is_some());
        assert!(track("video_timeline").is_none());
    }

    #[tokio::test]
    async fn a_rendition_sends_its_samples_to_the_relay() {
        // Arrange
        let mut relay = FakeRelay::spawn("rendition-samples");
        let broadcast = Broadcast::new(relay.connection(), vec!["live".to_string()]);
        let writer = BroadcastWriter::run(broadcast, SourceTracks::default());
        let mut rendition = RenditionTrack::new(
            &writer,
            RenditionConfig {
                track: VIDEO_TRACK_NAME.to_string(),
                label: "Video".to_string(),
                bitrate: None,
                framerate: None,
                cmaf: false,
                keyframes: RenditionKeyframes::SameFrames,
            },
        );
        rendition
            .push(MediaEvent::VideoConfig(vp8_config()))
            .await
            .unwrap();

        // Act
        rendition.push(keyframe_at(40_000)).await.unwrap();

        // Assert
        let group_id = writer.shared_timing().alignment.keyframe(40_000);
        relay.wait_for_video_group(group_id).await;
    }

    #[tokio::test]
    async fn the_cmaf_rendition_decodes_at_the_capture_timestamp_of_its_loc_sample() {
        // Arrange
        let mut relay = FakeRelay::spawn("rendition-cmaf-clock");
        let broadcast = Broadcast::new(relay.connection(), vec!["live".to_string()]);
        let writer = BroadcastWriter::run(broadcast, SourceTracks::default());
        let mut rendition = RenditionTrack::new(
            &writer,
            RenditionConfig {
                cmaf: true,
                ..rendition_config(RenditionKeyframes::SameFrames)
            },
        );
        rendition
            .push(MediaEvent::VideoConfig(vp8_config()))
            .await
            .unwrap();
        let subscription = relay.subscribe("live", "video_low_cmaf").await;

        // Act: a fragment is muxed once the next sample gives its duration
        rendition.push(keyframe_at(40_000)).await.unwrap();
        rendition.push(keyframe_at(80_000)).await.unwrap();

        // Assert
        let fragment = relay.first_object(&subscription).await;
        let init = Fmp4TrackMuxer::video(vp8_config()).init_segment().unwrap();
        let decoded = decode_times(&init, &[fragment.payload]);
        let captured = writer
            .shared_timing()
            .seeded_capture_origin()
            .unwrap()
            .saturating_add(Timestamp::from_micros(40_000));
        assert_eq!(decoded.len(), 1);
        assert!(decoded[0].micros().abs_diff(captured.micros()) <= VIDEO_TICK_US);
    }
}
