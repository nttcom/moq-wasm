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

fn rows<'a, T>(
    snapshots: &'a [RelaySnapshot],
    stats_of: impl Fn(&'a RelaySnapshot) -> &'a [T],
) -> Vec<Row<'a, T>> {
    snapshots
        .iter()
        .flat_map(|snapshot| {
            stats_of(snapshot).iter().map(|stats| Row {
                relay_id: &snapshot.relay_id,
                timestamp_ms: snapshot.timestamp_ms,
                stats,
            })
        })
        .collect()
}

pub async fn store(clickhouse: &ClickHouse, snapshots: &[RelaySnapshot]) -> anyhow::Result<()> {
    let snapshot_rows = snapshots
        .iter()
        .map(|snapshot| {
            Ok(SnapshotRow {
                relay_id: &snapshot.relay_id,
                timestamp_ms: snapshot.timestamp_ms,
                payload: String::from_utf8(snapshot.to_json())?,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let process_rows = rows(snapshots, |snapshot| {
        std::slice::from_ref(&snapshot.process)
    });
    let session_rows = rows(snapshots, |snapshot| &snapshot.sessions);
    let track_rows = rows(snapshots, |snapshot| &snapshot.tracks);
    let subscription_rows = rows(snapshots, |snapshot| &snapshot.subscriptions);
    tokio::try_join!(
        clickhouse.insert(SNAPSHOTS, &snapshot_rows),
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

    fn snapshot(relay_id: &str, timestamp_ms: u64, rss_bytes: u64) -> RelaySnapshot {
        RelaySnapshot {
            relay_id: relay_id.to_string(),
            timestamp_ms,
            process: ProcessStats {
                rss_bytes: Some(rss_bytes),
                cache_tracks: 2,
                cache_objects: 3,
                cache_payload_bytes: 4,
            },
            sessions: vec![],
            tracks: vec![],
            subscriptions: vec![],
        }
    }

    #[test]
    fn a_row_carries_the_snapshot_key_next_to_the_flattened_stats() {
        // Arrange
        let snapshots = [snapshot("relay-a", 42, 1)];

        // Act
        let rows = rows(&snapshots, |snapshot| {
            std::slice::from_ref(&snapshot.process)
        });

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

    #[test]
    fn every_snapshot_in_a_batch_keys_its_own_rows() {
        // Arrange
        let snapshots = [snapshot("relay-a", 1, 10), snapshot("relay-b", 2, 20)];

        // Act
        let rows = rows(&snapshots, |snapshot| {
            std::slice::from_ref(&snapshot.process)
        });

        // Assert
        let keys: Vec<(&str, u64, Option<u64>)> = rows
            .iter()
            .map(|row| (row.relay_id, row.timestamp_ms, row.stats.rss_bytes))
            .collect();
        assert_eq!(keys, [("relay-a", 1, Some(10)), ("relay-b", 2, Some(20))]);
    }
}
