use crate::{
    mp4::isobmff::VIDEO_TIMESCALE,
    sample::{Timestamp, VideoSample},
};

/// Holds each video sample until the next one arrives, because a sample's
/// duration is the gap between the two decode times.
#[derive(Default)]
pub(super) struct HeldVideo {
    pending: Option<VideoSample>,
    last_duration: u32,
}

impl HeldVideo {
    /// The previous sample and its duration in `VIDEO_TIMESCALE` ticks. A
    /// sample whose decode time is not after the previous one moves to one
    /// tick after it: a LOC publisher can repeat a capture time, and
    /// GStreamer's `av1parse` drops the PTS of a buffer that repeats the
    /// previous PTS.
    pub(super) fn push(&mut self, sample: &VideoSample) -> Option<(VideoSample, u32)> {
        let mut sample = sample.clone();
        if let Some(previous) = &self.pending {
            let previous_ticks = previous.dts.ticks(VIDEO_TIMESCALE);
            if sample.dts.ticks(VIDEO_TIMESCALE) <= previous_ticks {
                let dts = Timestamp::from_ticks(previous_ticks + 1, VIDEO_TIMESCALE);
                sample.pts = sample.pts.saturating_add(dts.saturating_sub(sample.dts));
                sample.dts = dts;
            }
        }
        let next_dts = sample.dts;
        let previous = self.pending.replace(sample)?;
        self.last_duration = next_dts.saturating_sub(previous.dts).ticks(VIDEO_TIMESCALE) as u32;
        Some((previous, self.last_duration))
    }

    /// The held sample, lasting as long as the one before it.
    pub(super) fn finish(&mut self) -> Option<(VideoSample, u32)> {
        let sample = self.pending.take()?;
        Some((sample, self.last_duration))
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;

    fn sample(dts_micros: u64) -> VideoSample {
        VideoSample {
            data: Bytes::new(),
            is_keyframe: false,
            pts: Timestamp::from_micros(dts_micros + 40_000),
            dts: Timestamp::from_micros(dts_micros),
        }
    }

    fn times(sample: &VideoSample) -> (u64, u64) {
        (sample.dts.micros(), sample.pts.micros())
    }

    #[test]
    fn moves_a_sample_that_repeats_the_decode_time_one_tick_later() {
        // Arrange
        let mut held = HeldVideo::default();

        // Act
        held.push(&sample(0));
        let (first, first_duration) = held.push(&sample(0)).unwrap();
        let (repeated, repeated_duration) = held.push(&sample(50_000)).unwrap();

        // Assert: the repeated sample keeps its composition offset
        assert_eq!((times(&first), first_duration), ((0, 40_000), 1));
        assert_eq!((times(&repeated), repeated_duration), ((11, 40_011), 4_499));
    }
}
