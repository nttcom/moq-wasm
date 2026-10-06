use relay_stats::RelaySnapshot;
use serde::Serialize;

use crate::{
    clickhouse::ClickHouse,
    schema::{PROCESS_STATS, SESSION_STATS, SNAPSHOTS, SUBSCRIPTION_STATS, TRACK_STATS},
};

#[derive(Serialize)]
struct Row<'a, T> {
    relay_id: &'a str,
    timestamp_ms: u64,
    #[serde(flatten)]
    stats: &'a T,
}

#[derive(Serialize)]
struct SnapshotRow<'a> {
    relay_id: &'a str,
    timestamp_ms: u64,
    payload: String,
}

fn rows<'a, T>(snapshot: &'a RelaySnapshot, stats: &'a [T]) -> Vec<Row<'a, T>> {
    stats
        .iter()
        .map(|stats| Row {
            relay_id: &snapshot.relay_id,
            timestamp_ms: snapshot.timestamp_ms,
            stats,
        })
        .collect()
}

pub async fn store(clickhouse: &ClickHouse, snapshot: &RelaySnapshot) -> anyhow::Result<()> {
    let snapshot_row = [SnapshotRow {
        relay_id: &snapshot.relay_id,
        timestamp_ms: snapshot.timestamp_ms,
        payload: String::from_utf8(snapshot.to_json())?,
    }];
    let process_rows = rows(snapshot, std::slice::from_ref(&snapshot.process));
    let session_rows = rows(snapshot, &snapshot.sessions);
    let track_rows = rows(snapshot, &snapshot.tracks);
    let subscription_rows = rows(snapshot, &snapshot.subscriptions);
    tokio::try_join!(
        clickhouse.insert(SNAPSHOTS, &snapshot_row),
        clickhouse.insert(PROCESS_STATS, &process_rows),
        clickhouse.insert(SESSION_STATS, &session_rows),
        clickhouse.insert(TRACK_STATS, &track_rows),
        clickhouse.insert(SUBSCRIPTION_STATS, &subscription_rows),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use relay_stats::{ProcessStats, RelaySnapshot};
    use serde_json::json;

    use super::rows;

    #[test]
    fn a_row_carries_the_snapshot_key_next_to_the_flattened_stats() {
        // Arrange
        let snapshot = RelaySnapshot {
            relay_id: "relay-a".to_string(),
            timestamp_ms: 42,
            process: ProcessStats {
                rss_bytes: Some(1),
                cache_tracks: 2,
                cache_objects: 3,
                cache_payload_bytes: 4,
            },
            sessions: vec![],
            tracks: vec![],
            subscriptions: vec![],
        };

        // Act
        let rows = rows(&snapshot, std::slice::from_ref(&snapshot.process));

        // Assert
        assert_eq!(
            serde_json::to_value(&rows).unwrap(),
            json!([{
                "relay_id": "relay-a",
                "timestamp_ms": 42,
                "rss_bytes": 1,
                "cache_tracks": 2,
                "cache_objects": 3,
                "cache_payload_bytes": 4,
            }])
        );
    }
}
