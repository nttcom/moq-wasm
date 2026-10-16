use relay_stats::{ProcessStats, RelaySnapshot};

pub(crate) fn snapshot(timestamp_ms: u64) -> RelaySnapshot {
    RelaySnapshot {
        relay_id: "relay-a".to_string(),
        timestamp_ms,
        process: ProcessStats {
            rss_bytes: None,
            cache_tracks: 0,
            cache_objects: 0,
            cache_payload_bytes: 0,
            disk_cache_bytes: 0,
            disk_cache_max_bytes: 0,
            client_sessions: 0,
            draining_since_ms: None,
        },
        sessions: vec![],
        tracks: vec![],
        subscriptions: vec![],
    }
}
