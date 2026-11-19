use anyhow::Result;
use mediapack::{MediaEvent, VideoConfig, VideoSample, mp4::Fmp4TrackMuxer};
use moqt::publish::{GroupBoundary, OutgoingObject};

/// The CMAF sibling of a LOC video track. The muxer releases each sample's
/// fragment when the next sample arrives, and moves a sample whose decode time
/// repeats the previous one, presentation time included, one tick later, so a
/// fragment starts the group its sample started on the LOC track rather than
/// one looked up by the fragment's presentation time.
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

    /// `group` is the boundary `sample` took on the LOC track. Returns the
    /// fragment of the sample pushed before it.
    pub(crate) fn push(
        &mut self,
        sample: &VideoSample,
        group: GroupBoundary,
    ) -> Result<Option<OutgoingObject>> {
        let fragment = self.muxer.push(&MediaEvent::Video(sample.clone()))?;
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
        cmaf.push(&sample(0, false), GroupBoundary::Within).unwrap();

        // Act
        let delta = cmaf.push(&sample(0, true), GroupBoundary::At(7)).unwrap();
        let keyframe = cmaf
            .push(&sample(40_000, false), GroupBoundary::Within)
            .unwrap();

        // Assert
        assert_eq!(delta.unwrap().group, GroupBoundary::Within);
        assert_eq!(keyframe.unwrap().group, GroupBoundary::At(7));
    }
}
