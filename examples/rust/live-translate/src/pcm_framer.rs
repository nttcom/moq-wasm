pub(crate) struct PcmFramer {
    samples_per_frame: usize,
    pending: Vec<i16>,
}

impl PcmFramer {
    pub(crate) fn new(samples_per_frame: usize) -> Self {
        Self {
            samples_per_frame,
            pending: Vec::with_capacity(samples_per_frame),
        }
    }

    pub(crate) fn push(&mut self, samples: &[i16]) -> Vec<Vec<i16>> {
        self.pending.extend_from_slice(samples);
        let complete = self.pending.len() / self.samples_per_frame * self.samples_per_frame;
        let rest = self.pending.split_off(complete);
        let frames = self
            .pending
            .chunks_exact(self.samples_per_frame)
            .map(<[i16]>::to_vec)
            .collect();
        self.pending = rest;
        frames
    }

    pub(crate) fn flush_padded(&mut self) -> Option<Vec<i16>> {
        if self.pending.is_empty() {
            return None;
        }
        let mut frame = std::mem::take(&mut self.pending);
        frame.resize(self.samples_per_frame, 0);
        Some(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_beyond_whole_frames_wait_for_the_next_push() {
        // Arrange
        let mut framer = PcmFramer::new(4);

        // Act
        let first = framer.push(&[1, 2, 3, 4, 5, 6]);
        let second = framer.push(&[7, 8]);

        // Assert
        assert_eq!(first, vec![vec![1, 2, 3, 4]]);
        assert_eq!(second, vec![vec![5, 6, 7, 8]]);
    }

    #[test]
    fn flush_pads_the_partial_frame_with_silence() {
        // Arrange
        let mut framer = PcmFramer::new(4);
        framer.push(&[1, 2, 3, 4, 5]);

        // Act
        let flushed = framer.flush_padded();

        // Assert
        assert_eq!(flushed, Some(vec![5, 0, 0, 0]));
        assert_eq!(framer.flush_padded(), None);
    }
}
