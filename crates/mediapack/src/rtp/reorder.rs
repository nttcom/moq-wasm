use std::collections::BTreeMap;

use bytes::Bytes;

use crate::rtp::packet::{Header, extend_wrapping};

/// Packets held back behind a gap are released as lost once the held
/// timestamps span this much of the stream, or once this many are pending.
const MAX_SPAN_MILLIS: u64 = 200;
const MAX_PENDING: usize = 256;

/// Puts the packets of one stream back into sequence order and reports the
/// packets that never arrived. Sequence numbers are extended past the 16-bit
/// wrap; a packet behind the position already released is dropped as late.
pub(crate) struct ReorderBuffer {
    max_span_ticks: u64,
    next: Option<u64>,
    highest: Option<u64>,
    pending: BTreeMap<u64, (Header, Bytes)>,
}

pub(crate) enum Ordered {
    Packet(Header, Bytes),
    /// Packets before the next one were lost.
    Lost,
}

impl ReorderBuffer {
    pub(crate) fn new(clock_rate: u32) -> Self {
        Self {
            max_span_ticks: clock_rate as u64 * MAX_SPAN_MILLIS / 1_000,
            next: None,
            highest: None,
            pending: BTreeMap::new(),
        }
    }

    pub(crate) fn push(&mut self, header: Header, payload: Bytes) -> Vec<Ordered> {
        let sequence = extend_wrapping(&mut self.highest, header.sequence_number as u64, 16);
        let next = *self.next.get_or_insert(sequence);
        if sequence < next || self.pending.contains_key(&sequence) {
            return Vec::new();
        }
        self.pending.insert(sequence, (header, payload));
        self.release(false)
    }

    pub(crate) fn finish(&mut self) -> Vec<Ordered> {
        self.release(true)
    }

    fn release(&mut self, drain: bool) -> Vec<Ordered> {
        let mut ordered = Vec::new();
        let Some(mut next) = self.next else {
            return ordered;
        };
        while let Some(first) = self.pending.keys().next().copied() {
            if first != next {
                if !(drain || self.gap_expired()) {
                    break;
                }
                ordered.push(Ordered::Lost);
                next = first;
            }
            let (header, payload) = self.pending.remove(&first).expect("first key exists");
            ordered.push(Ordered::Packet(header, payload));
            next += 1;
        }
        self.next = Some(next);
        ordered
    }

    fn gap_expired(&self) -> bool {
        if self.pending.len() >= MAX_PENDING {
            return true;
        }
        let (Some((_, (oldest, _))), Some((_, (newest, _)))) = (
            self.pending.first_key_value(),
            self.pending.last_key_value(),
        ) else {
            return false;
        };
        newest.timestamp.wrapping_sub(oldest.timestamp) as u64 >= self.max_span_ticks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(sequence_number: u16, timestamp: u32) -> Header {
        Header {
            marker: false,
            payload_type: 96,
            sequence_number,
            timestamp,
            ssrc: 1,
        }
    }

    fn sequences(ordered: &[Ordered]) -> Vec<i64> {
        ordered
            .iter()
            .map(|item| match item {
                Ordered::Packet(header, _) => header.sequence_number as i64,
                Ordered::Lost => -1,
            })
            .collect()
    }

    #[test]
    fn releases_packets_in_sequence_order() {
        // Arrange
        let mut buffer = ReorderBuffer::new(90_000);

        // Act
        let mut released = buffer.push(header(10, 0), Bytes::new());
        released.extend(buffer.push(header(12, 0), Bytes::new()));
        released.extend(buffer.push(header(11, 0), Bytes::new()));

        // Assert
        assert_eq!(sequences(&released), [10, 11, 12]);
    }

    #[test]
    fn drops_late_and_duplicate_packets() {
        // Arrange
        let mut buffer = ReorderBuffer::new(90_000);
        buffer.push(header(10, 0), Bytes::new());

        // Act
        let duplicate = buffer.push(header(10, 0), Bytes::new());
        let late = buffer.push(header(9, 0), Bytes::new());

        // Assert
        assert!(duplicate.is_empty());
        assert!(late.is_empty());
    }

    #[test]
    fn reports_loss_once_the_held_packets_span_the_limit() {
        // Arrange: 90 kHz, so 18 000 ticks are 200 ms
        let mut buffer = ReorderBuffer::new(90_000);
        buffer.push(header(1, 0), Bytes::new());

        // Act
        let held = buffer.push(header(3, 3_000), Bytes::new());
        let still_held = buffer.push(header(4, 6_000), Bytes::new());
        let released = buffer.push(header(5, 21_000), Bytes::new());

        // Assert
        assert!(held.is_empty());
        assert!(still_held.is_empty());
        assert_eq!(sequences(&released), [-1, 3, 4, 5]);
    }

    #[test]
    fn finish_releases_everything_and_reports_the_gaps() {
        // Arrange
        let mut buffer = ReorderBuffer::new(48_000);
        buffer.push(header(100, 0), Bytes::new());
        buffer.push(header(104, 0), Bytes::new());

        // Act
        let released = buffer.finish();

        // Assert
        assert_eq!(sequences(&released), [-1, 104]);
    }

    #[test]
    fn extends_sequence_numbers_across_the_wrap() {
        // Arrange
        let mut buffer = ReorderBuffer::new(90_000);

        // Act
        let mut released = buffer.push(header(65_534, 0), Bytes::new());
        released.extend(buffer.push(header(65_535, 0), Bytes::new()));
        released.extend(buffer.push(header(1, 0), Bytes::new()));
        released.extend(buffer.push(header(0, 0), Bytes::new()));

        // Assert
        assert_eq!(sequences(&released), [65_534, 65_535, 0, 1]);
    }
}
