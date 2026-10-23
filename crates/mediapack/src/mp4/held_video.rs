use crate::{mp4::isobmff::VIDEO_TIMESCALE, sample::VideoSample};

/// Holds each video sample until the next one arrives, because a sample's
/// duration is the gap between the two decode times.
#[derive(Default)]
pub(super) struct HeldVideo {
    pending: Option<VideoSample>,
    last_duration: u32,
}

impl HeldVideo {
    /// The previous sample and its duration in `VIDEO_TIMESCALE` ticks, once
    /// `sample` gives it.
    pub(super) fn push(&mut self, sample: &VideoSample) -> Option<(VideoSample, u32)> {
        let previous = self.pending.replace(sample.clone())?;
        self.last_duration = sample
            .dts
            .saturating_sub(previous.dts)
            .ticks(VIDEO_TIMESCALE) as u32;
        Some((previous, self.last_duration))
    }

    /// The held sample, lasting as long as the one before it.
    pub(super) fn finish(&mut self) -> Option<(VideoSample, u32)> {
        let sample = self.pending.take()?;
        Some((sample, self.last_duration))
    }
}
