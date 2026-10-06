use std::sync::{
    Mutex, PoisonError,
    atomic::{AtomicU64, Ordering},
};

use tokio::time::Instant;

#[derive(Default)]
pub(crate) struct DeliveryStats {
    streams_opened: AtomicU64,
    streams_reset: AtomicU64,
    objects_sent: AtomicU64,
    bytes_sent: AtomicU64,
    last_sent_received_at: Mutex<Option<Instant>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeliveryCounters {
    pub(crate) streams_opened: u64,
    pub(crate) streams_reset: u64,
    pub(crate) objects_sent: u64,
    pub(crate) bytes_sent: u64,
    pub(crate) last_sent_received_at: Option<Instant>,
}

impl DeliveryStats {
    pub(crate) fn record_stream_opened(&self) {
        self.streams_opened.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn record_stream_reset(&self) {
        self.streams_reset.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_object_sent(&self, payload_bytes: usize, received_at: Instant) {
        self.objects_sent.fetch_add(1, Ordering::Relaxed);
        self.bytes_sent
            .fetch_add(payload_bytes as u64, Ordering::Relaxed);
        let mut last = self
            .last_sent_received_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *last = Some(last.map_or(received_at, |previous| previous.max(received_at)));
    }

    pub(crate) fn streams_opened(&self) -> u64 {
        self.streams_opened.load(Ordering::Acquire)
    }

    pub(crate) fn counters(&self) -> DeliveryCounters {
        DeliveryCounters {
            streams_opened: self.streams_opened(),
            streams_reset: self.streams_reset.load(Ordering::Relaxed),
            objects_sent: self.objects_sent.load(Ordering::Relaxed),
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            last_sent_received_at: *self
                .last_sent_received_at
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::time::Instant;

    use super::{DeliveryCounters, DeliveryStats};

    #[test]
    fn counters_accumulate_every_record() {
        // Arrange
        let stats = DeliveryStats::default();
        let received_at = Instant::now();

        // Act
        stats.record_stream_opened();
        stats.record_stream_opened();
        stats.record_stream_reset();
        stats.record_object_sent(10, received_at);
        stats.record_object_sent(5, received_at);

        // Assert
        assert_eq!(
            stats.counters(),
            DeliveryCounters {
                streams_opened: 2,
                streams_reset: 1,
                objects_sent: 2,
                bytes_sent: 15,
                last_sent_received_at: Some(received_at),
            }
        );
    }

    #[test]
    fn an_older_object_sent_late_does_not_move_the_delivery_position_back() {
        // Arrange
        let stats = DeliveryStats::default();
        let newer = Instant::now();
        let older = newer - Duration::from_millis(500);

        // Act
        stats.record_object_sent(1, newer);
        stats.record_object_sent(1, older);

        // Assert
        assert_eq!(stats.counters().last_sent_received_at, Some(newer));
    }
}
