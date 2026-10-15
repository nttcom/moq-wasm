use std::{
    collections::BTreeMap,
    sync::{Arc, PoisonError, RwLock},
    time::Duration,
};

use relay_stats::RelaySnapshot;
use tokio::time::Instant;

/// A relay that sent no snapshot for this long has stopped or lost its
/// stats session, and is left out of the topology.
pub const RELAY_SILENCE: Duration = Duration::from_secs(10);

#[derive(Clone, Default)]
pub struct LatestSnapshots {
    by_relay: Arc<RwLock<BTreeMap<String, (RelaySnapshot, Instant)>>>,
}

impl LatestSnapshots {
    pub fn update(&self, snapshot: RelaySnapshot) {
        self.by_relay
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(snapshot.relay_id.clone(), (snapshot, Instant::now()));
    }

    /// Ages each relay by when its snapshot arrived here, not by the relay's
    /// clock.
    pub fn current(&self) -> Vec<RelaySnapshot> {
        self.by_relay
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|(_, received_at)| received_at.elapsed() <= RELAY_SILENCE)
            .map(|(snapshot, _)| snapshot.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use relay_stats::{ProcessStats, RelaySnapshot};

    use super::{LatestSnapshots, RELAY_SILENCE};

    fn snapshot(relay_id: &str) -> RelaySnapshot {
        RelaySnapshot {
            relay_id: relay_id.to_string(),
            timestamp_ms: 1,
            process: ProcessStats::default(),
            sessions: vec![],
            tracks: vec![],
            subscriptions: vec![],
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_relay_that_stopped_sending_leaves_the_current_snapshots() {
        // Arrange: relay-a went silent, relay-b keeps sending
        let latest = LatestSnapshots::default();
        latest.update(snapshot("relay-a"));
        tokio::time::advance(RELAY_SILENCE).await;
        latest.update(snapshot("relay-b"));
        tokio::time::advance(std::time::Duration::from_secs(1)).await;

        // Act
        let current = latest.current();

        // Assert
        let relays: Vec<&str> = current
            .iter()
            .map(|snapshot| snapshot.relay_id.as_str())
            .collect();
        assert_eq!(relays, vec!["relay-b"]);
    }
}
