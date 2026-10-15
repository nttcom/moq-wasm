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

/// JSONEachRow bodies of the rows that snapshots add to each table, one string per table.
#[derive(Default)]
pub(crate) struct SnapshotRows {
    snapshots: String,
    process_stats: String,
    session_stats: String,
    track_stats: String,
    subscription_stats: String,
}

fn append<T: Serialize>(
    body: &mut String,
    rows: impl IntoIterator<Item = T>,
) -> anyhow::Result<()> {
    for row in rows {
        body.push_str(&serde_json::to_string(&row)?);
        body.push('\n');
    }
    Ok(())
}

impl SnapshotRows {
    pub(crate) fn push(&mut self, snapshot: &RelaySnapshot) -> anyhow::Result<()> {
        append(
            &mut self.snapshots,
            [SnapshotRow {
                relay_id: &snapshot.relay_id,
                timestamp_ms: snapshot.timestamp_ms,
                payload: String::from_utf8(snapshot.to_json())?,
            }],
        )?;
        append(
            &mut self.process_stats,
            rows(snapshot, std::slice::from_ref(&snapshot.process)),
        )?;
        append(&mut self.session_stats, rows(snapshot, &snapshot.sessions))?;
        append(&mut self.track_stats, rows(snapshot, &snapshot.tracks))?;
        append(
            &mut self.subscription_stats,
            rows(snapshot, &snapshot.subscriptions),
        )
    }

    /// Inserts every table's rows in one INSERT per table.
    pub(crate) async fn store(self, clickhouse: &ClickHouse) -> anyhow::Result<()> {
        tokio::try_join!(
            clickhouse.insert_json_lines(SNAPSHOTS, self.snapshots),
            clickhouse.insert_json_lines(PROCESS_STATS, self.process_stats),
            clickhouse.insert_json_lines(SESSION_STATS, self.session_stats),
            clickhouse.insert_json_lines(TRACK_STATS, self.track_stats),
            clickhouse.insert_json_lines(SUBSCRIPTION_STATS, self.subscription_stats),
        )
        .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use relay_stats::{ProcessStats, RelaySnapshot};
    use serde_json::json;

    use super::{SnapshotRows, rows};

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
                disk_cache_bytes: 5,
                client_sessions: 6,
                draining_since_ms: Some(7),
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
                "disk_cache_bytes": 5,
                "client_sessions": 6,
                "draining_since_ms": 7,
            }])
        );
    }

    fn snapshot(timestamp_ms: u64) -> RelaySnapshot {
        RelaySnapshot {
            relay_id: "relay-a".to_string(),
            timestamp_ms,
            process: ProcessStats {
                rss_bytes: None,
                cache_tracks: 0,
                cache_objects: 0,
                cache_payload_bytes: 0,
                disk_cache_bytes: 0,
                client_sessions: 0,
                draining_since_ms: None,
            },
            sessions: vec![],
            tracks: vec![],
            subscriptions: vec![],
        }
    }

    #[test]
    fn pushed_snapshots_become_one_line_each_per_table() {
        // Arrange
        let mut buffered = SnapshotRows::default();

        // Act
        buffered.push(&snapshot(1)).unwrap();
        buffered.push(&snapshot(2)).unwrap();

        // Assert: a snapshot without sessions adds no session row
        let timestamps = |body: &str| -> Vec<u64> {
            body.lines()
                .map(|line| {
                    serde_json::from_str::<serde_json::Value>(line).unwrap()["timestamp_ms"]
                        .as_u64()
                        .unwrap()
                })
                .collect()
        };
        assert_eq!(timestamps(&buffered.snapshots), [1, 2]);
        assert_eq!(timestamps(&buffered.process_stats), [1, 2]);
        assert!(buffered.session_stats.is_empty());
    }
}
