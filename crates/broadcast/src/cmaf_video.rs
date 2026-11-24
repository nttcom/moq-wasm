use anyhow::Result;
use mediapack::{MediaEvent, Timestamp, VideoConfig, VideoSample, mp4::Fmp4TrackMuxer};
use moqt::publish::{GroupBoundary, OutgoingObject};

/// The CMAF sibling of a LOC video track, on the LOC track's capture clock.
/// The muxer releases each sample's fragment when the next sample arrives, and
/// moves a sample whose decode time repeats the previous one, presentation time
/// included, one tick later, so a fragment starts the group its sample started
/// on the LOC track rather than one looked up by the fragment's presentation
/// time.
pub(crate) struct CmafVideo {
    muxer: Fmp4TrackMuxer,
    held_group: GroupBoundary,
}

impl CmafVideo {
    pub(crate) fn new(config: VideoConfig) -> Self {
        Self {
            muxer: Fmp4TrackMuxer::video(config),
            held_group: GroupBoundary::Within,
        }
    }

    /// `group` is the boundary `sample` took on the LOC track, and
    /// `capture_origin` the wall-clock time of its presentation time zero.
    /// Returns the fragment of the sample pushed before it.
    pub(crate) fn push(
        &mut self,
        sample: &VideoSample,
        group: GroupBoundary,
        capture_origin: Timestamp,
    ) -> Result<Option<OutgoingObject>> {
        let captured = VideoSample {
            pts: capture_origin.saturating_add(sample.pts),
            dts: capture_origin.saturating_add(sample.dts),
            ..sample.clone()
        };
        let fragment = self.muxer.push(&MediaEvent::Video(captured))?;
        let held_group = std::mem::replace(&mut self.held_group, group);
        Ok(fragment.map(|fragment| OutgoingObject::plain(held_group, fragment.data)))
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use mediapack::{Timestamp, vp8::Vp8Config};

    use super::*;

    fn sample(dts_us: u64, is_keyframe: bool) -> VideoSample {
        VideoSample {
            data: Bytes::from_static(b"frame"),
            is_keyframe,
            pts: Timestamp::from_micros(dts_us),
            dts: Timestamp::from_micros(dts_us),
        }
    }

    #[test]
    fn a_fragment_starts_the_group_its_sample_was_pushed_with_though_its_decode_time_repeats() {
        // Arrange
        let mut cmaf = CmafVideo::new(VideoConfig::Vp8(Vp8Config {
            width: 64,
            height: 48,
        }));
        cmaf.push(&sample(0, false), GroupBoundary::Within, Timestamp::ZERO)
            .unwrap();

        // Act
        let delta = cmaf
            .push(&sample(0, true), GroupBoundary::At(7), Timestamp::ZERO)
            .unwrap();
        let keyframe = cmaf
            .push(
                &sample(40_000, false),
                GroupBoundary::Within,
                Timestamp::ZERO,
            )
            .unwrap();

        // Assert
        assert_eq!(delta.unwrap().group, GroupBoundary::Within);
        assert_eq!(keyframe.unwrap().group, GroupBoundary::At(7));
    }

    mod properties {
        use proptest::prelude::*;

        use super::*;
        use crate::test_support::{VIDEO_TICK_US, decode_times, vp8_config};

        /// Capture origins from zero through today's wall clock to the largest
        /// integer a JavaScript player reads exactly.
        fn capture_origin_us() -> impl Strategy<Value = u64> {
            prop_oneof![
                Just(0),
                1_700_000_000_000_000..2_000_000_000_000_000u64,
                Just(1 << 53),
            ]
        }

        /// Gaps of one to three ticks, a frame at 30 fps and long pauses. No
        /// gap is zero: the muxer moves a repeated decode time one tick later.
        fn gap_us() -> impl Strategy<Value = u64> {
            prop_oneof![VIDEO_TICK_US..=34, Just(33_333), 1_000_000..=60_000_000u64]
        }

        proptest! {
            #[test]
            fn a_fragment_decodes_at_the_capture_timestamp_of_its_sample(
                origin_us in capture_origin_us(),
                gaps in prop::collection::vec(gap_us(), 2..20),
            ) {
                // Arrange
                let origin = Timestamp::from_micros(origin_us);
                let mut cmaf = CmafVideo::new(vp8_config());
                let dts_us: Vec<u64> = gaps
                    .iter()
                    .scan(0, |dts, gap| {
                        *dts += gap;
                        Some(*dts)
                    })
                    .collect();

                // Act
                let fragments: Vec<Bytes> = dts_us
                    .iter()
                    .filter_map(|&dts| {
                        cmaf.push(&sample(dts, true), GroupBoundary::Within, origin)
                            .unwrap()
                    })
                    .map(|object| object.payload)
                    .collect();

                // Assert: every sample but the last, which the muxer still holds, decodes within a tick of its LOC Timestamp
                let init = Fmp4TrackMuxer::video(vp8_config()).init_segment().unwrap();
                let decoded = decode_times(&init, &fragments);
                prop_assert_eq!(decoded.len(), dts_us.len() - 1);
                for (decoded, dts) in decoded.iter().zip(&dts_us) {
                    let captured = origin.saturating_add(Timestamp::from_micros(*dts));
                    prop_assert!(
                        decoded.micros().abs_diff(captured.micros()) <= VIDEO_TICK_US,
                        "{decoded:?} is not within a tick of {captured:?}"
                    );
                }
            }
        }
    }
}
