use std::time::Duration;

use super::translation_timeline::{FRAME_DURATION, TranslationTimeline};
use crate::pcm_framer::PcmFramer;

#[derive(Debug, PartialEq)]
pub(crate) struct TranslatedFrame {
    pub(crate) timestamp_micros: u64,
    pub(crate) samples: Vec<i16>,
}

pub(crate) struct TranslatedAudioSequencer {
    timeline: TranslationTimeline,
    framer: PcmFramer,
}

impl TranslatedAudioSequencer {
    pub(crate) fn new(gemini_delay: Duration, output_sample_rate: u32) -> Self {
        let samples_per_frame = usize::try_from(
            u128::from(output_sample_rate) * FRAME_DURATION.as_micros() / 1_000_000,
        )
        .unwrap_or(usize::MAX);
        Self {
            timeline: TranslationTimeline::new(gemini_delay),
            framer: PcmFramer::new(samples_per_frame),
        }
    }

    pub(crate) fn record_input(&mut self, timestamp_micros: u64) {
        self.timeline.record_input(timestamp_micros);
    }

    pub(crate) fn restart(&mut self) -> Vec<TranslatedFrame> {
        let frames = self
            .framer
            .flush_padded()
            .and_then(|samples| self.stamp(samples))
            .into_iter()
            .collect();
        self.timeline.restart();
        frames
    }

    pub(crate) fn push_chunk(&mut self, samples: &[i16]) -> Vec<TranslatedFrame> {
        self.framer
            .push(samples)
            .into_iter()
            .filter_map(|samples| self.stamp(samples))
            .collect()
    }

    fn stamp(&mut self, samples: Vec<i16>) -> Option<TranslatedFrame> {
        Some(TranslatedFrame {
            timestamp_micros: self.timeline.next_frame_timestamp_micros()?,
            samples,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GEMINI_DELAY: Duration = Duration::from_millis(3_700);
    const SAMPLE_RATE: u32 = 24_000;
    const SAMPLES_PER_FRAME: usize = 480;
    const CHUNK_SAMPLES: usize = 6_000;

    fn timestamps(frames: &[TranslatedFrame]) -> Vec<u64> {
        frames.iter().map(|frame| frame.timestamp_micros).collect()
    }

    #[test]
    fn chunks_of_one_connection_follow_each_other_even_after_a_delivery_stall() {
        // Arrange
        let mut sequencer = TranslatedAudioSequencer::new(GEMINI_DELAY, SAMPLE_RATE);
        sequencer.record_input(105_200_000);
        let first = sequencer.push_chunk(&vec![1; CHUNK_SAMPLES]);
        sequencer.record_input(107_500_000);

        // Act
        let after_stall = sequencer.push_chunk(&vec![1; CHUNK_SAMPLES]);

        // Assert: the stalled chunk continues where the first one ended
        assert_eq!(timestamps(&first).first(), Some(&101_500_000));
        assert_eq!(timestamps(&first).last(), Some(&101_720_000));
        assert_eq!(timestamps(&after_stall).first(), Some(&101_740_000));
    }

    #[test]
    fn restart_flushes_the_partial_frame_then_starts_from_the_latest_input() {
        // Arrange
        let mut sequencer = TranslatedAudioSequencer::new(GEMINI_DELAY, SAMPLE_RATE);
        sequencer.record_input(105_200_000);
        sequencer.push_chunk(&vec![1; SAMPLES_PER_FRAME + 10]);
        sequencer.record_input(114_700_000);

        // Act
        let flushed = sequencer.restart();
        let after_restart = sequencer.push_chunk(&vec![2; SAMPLES_PER_FRAME]);

        // Assert
        assert_eq!(timestamps(&flushed), vec![101_520_000]);
        assert_eq!(&flushed[0].samples[..10], &[1; 10]);
        assert_eq!(flushed[0].samples[10..], vec![0; SAMPLES_PER_FRAME - 10]);
        assert_eq!(timestamps(&after_restart), vec![111_000_000]);
    }
}
