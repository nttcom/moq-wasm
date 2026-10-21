use relay_stats::{ProcessStats, RelaySnapshot};

pub(crate) fn snapshot(timestamp_ms: u64) -> RelaySnapshot {
    RelaySnapshot {
        relay_id: "relay-a".to_string(),
        timestamp_ms,
        process: ProcessStats::default(),
        sessions: vec![],
        tracks: vec![],
        subscriptions: vec![],
    }
}
