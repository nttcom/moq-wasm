use std::collections::VecDeque;

use crate::media::g711::{G711, SAMPLES_PER_PACKET};

/// Audio is held back until this much is buffered, so the jitter of the
/// MoQT side does not leave gaps in the 20 ms RTP clock.
const TARGET_SAMPLES: usize = 2 * SAMPLES_PER_PACKET;
/// Above this the oldest audio is dropped down to the target, so a burst
/// after a stall does not delay the call for good.
const MAX_SAMPLES: usize = 6 * SAMPLES_PER_PACKET;

/// The G.711 audio waiting to go to the phone. The trunk gets one packet
/// every 20 ms whether audio arrived or not, so an empty or refilling buffer
/// plays silence.
pub(crate) struct Playout {
    codec: G711,
    samples: VecDeque<u8>,
    playing: bool,
}

impl Playout {
    pub(crate) fn new(codec: G711) -> Self {
        Self {
            codec,
            samples: VecDeque::with_capacity(MAX_SAMPLES + SAMPLES_PER_PACKET),
            playing: false,
        }
    }

    pub(crate) fn push(&mut self, encoded: &[u8]) {
        self.samples.extend(encoded);
        if self.samples.len() > MAX_SAMPLES {
            let excess = self.samples.len() - TARGET_SAMPLES;
            self.samples.drain(..excess);
        }
    }

    pub(crate) fn next_packet(&mut self) -> Vec<u8> {
        if !self.playing && self.samples.len() >= TARGET_SAMPLES {
            self.playing = true;
        }
        if !self.playing || self.samples.len() < SAMPLES_PER_PACKET {
            self.playing = false;
            return vec![self.codec.silence(); SAMPLES_PER_PACKET];
        }
        self.samples.drain(..SAMPLES_PER_PACKET).collect()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn audio_plays_once_the_target_is_buffered() {
        // Arrange
        let mut playout = Playout::new(G711::Pcmu);
        playout.push(&[1; SAMPLES_PER_PACKET]);

        // Act
        let before = playout.next_packet();
        playout.push(&[2; SAMPLES_PER_PACKET]);
        let after = playout.next_packet();

        // Assert
        assert_eq!(before, vec![0xFF; SAMPLES_PER_PACKET]);
        assert_eq!(after, vec![1; SAMPLES_PER_PACKET]);
    }

    #[test]
    fn a_burst_keeps_only_the_newest_audio() {
        // Arrange
        let mut playout = Playout::new(G711::Pcma);
        let burst: Vec<u8> = (0..7)
            .flat_map(|value| [value; SAMPLES_PER_PACKET])
            .collect();

        // Act
        playout.push(&burst);
        let first = playout.next_packet();

        // Assert: seven packets exceed the maximum, so only the last two stay
        assert_eq!(first, vec![5; SAMPLES_PER_PACKET]);
    }

    proptest! {
        /// Every packet is 20 ms long, the buffer never holds more than its
        /// maximum, and each packet of audio is a stretch of what was pushed,
        /// later than the one before it.
        #[test]
        fn packets_are_whole_bounded_and_in_order(
            steps in prop::collection::vec(
                prop_oneof![
                    (0usize..=3 * SAMPLES_PER_PACKET).prop_map(Some),
                    Just(None),
                ],
                0..200,
            )
        ) {
            // Arrange: values below 0xFF, so audio never looks like μ-law silence
            let mut playout = Playout::new(G711::Pcmu);
            let mut pushed: Vec<u8> = Vec::new();
            let mut played_until = 0;

            for step in steps {
                // Act
                match step {
                    Some(length) => {
                        let chunk: Vec<u8> =
                            (pushed.len()..pushed.len() + length).map(|index| (index % 251) as u8).collect();
                        pushed.extend(&chunk);
                        playout.push(&chunk);
                    }
                    None => {
                        let packet = playout.next_packet();

                        // Assert
                        prop_assert_eq!(packet.len(), SAMPLES_PER_PACKET);
                        if packet != vec![0xFF; SAMPLES_PER_PACKET] {
                            let start = (played_until..=pushed.len() - SAMPLES_PER_PACKET)
                                .find(|&start| pushed[start..start + SAMPLES_PER_PACKET] == packet[..]);
                            prop_assert!(start.is_some());
                            played_until = start.unwrap() + SAMPLES_PER_PACKET;
                        }
                    }
                }
                prop_assert!(playout.samples.len() <= MAX_SAMPLES);
            }
        }
    }
}
