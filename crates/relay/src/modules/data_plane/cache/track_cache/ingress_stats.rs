use std::{
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::time::Instant;

#[derive(Default)]
pub(crate) struct IngressStats {
    objects_received: AtomicU64,
    bytes_received: AtomicU64,
    subgroups_aborted: AtomicU64,
    arrivals: Mutex<Arrivals>,
}

#[derive(Default)]
struct Arrivals {
    last: Option<Instant>,
    max_gap_since_take: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IngressCounters {
    pub(crate) objects_received: u64,
    pub(crate) bytes_received: u64,
    pub(crate) subgroups_aborted: u64,
    pub(crate) max_arrival_gap: Duration,
    pub(crate) last_arrival: Option<Instant>,
}

impl IngressStats {
    pub(super) fn record_object(&self, payload_bytes: usize) {
        self.objects_received.fetch_add(1, Ordering::Relaxed);
        self.bytes_received
            .fetch_add(payload_bytes as u64, Ordering::Relaxed);
    }

    pub(super) fn record_live_arrival(&self, at: Instant) {
        let mut arrivals = self.arrivals.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(last) = arrivals.last {
            let gap = at.saturating_duration_since(last);
            arrivals.max_gap_since_take = arrivals.max_gap_since_take.max(gap);
        }
        arrivals.last = Some(at);
    }

    pub(super) fn record_aborted_subgroup(&self) {
        self.subgroups_aborted.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn take(&self) -> IngressCounters {
        let mut arrivals = self.arrivals.lock().unwrap_or_else(PoisonError::into_inner);
        IngressCounters {
            objects_received: self.objects_received.load(Ordering::Relaxed),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
            subgroups_aborted: self.subgroups_aborted.load(Ordering::Relaxed),
            max_arrival_gap: std::mem::take(&mut arrivals.max_gap_since_take),
            last_arrival: arrivals.last,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::time::Instant;

    use super::IngressStats;

    #[test]
    fn take_reports_cumulative_counts() {
        // Arrange
        let stats = IngressStats::default();
        stats.record_object(100);
        stats.record_object(20);
        stats.record_aborted_subgroup();

        // Act
        let counters = stats.take();

        // Assert
        assert_eq!(counters.objects_received, 2);
        assert_eq!(counters.bytes_received, 120);
        assert_eq!(counters.subgroups_aborted, 1);
    }

    #[test]
    fn max_arrival_gap_covers_only_the_arrivals_since_the_previous_take() {
        // Arrange
        let stats = IngressStats::default();
        let start = Instant::now();
        stats.record_live_arrival(start);
        stats.record_live_arrival(start + Duration::from_millis(300));
        stats.record_live_arrival(start + Duration::from_millis(340));
        let first = stats.take();
        stats.record_live_arrival(start + Duration::from_millis(390));

        // Act
        let second = stats.take();

        // Assert
        assert_eq!(first.max_arrival_gap, Duration::from_millis(300));
        assert_eq!(second.max_arrival_gap, Duration::from_millis(50));
        assert_eq!(
            second.last_arrival,
            Some(start + Duration::from_millis(390))
        );
    }
}
