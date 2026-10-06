pub const RETENTION_DAYS: u32 = 7;

pub const SNAPSHOTS: &str = "snapshots";
pub const PROCESS_STATS: &str = "process_stats";
pub const SESSION_STATS: &str = "session_stats";
pub const TRACK_STATS: &str = "track_stats";
pub const SUBSCRIPTION_STATS: &str = "subscription_stats";

const COUNTER: &str = "UInt64";

struct Table {
    name: &'static str,
    columns: &'static [(&'static str, &'static str)],
    order_by: &'static [&'static str],
}

const TABLES: &[Table] = &[
    Table {
        name: SNAPSHOTS,
        columns: &[("payload", "String")],
        order_by: &[],
    },
    Table {
        name: PROCESS_STATS,
        columns: &[
            ("rss_bytes", "Nullable(UInt64)"),
            ("cache_tracks", COUNTER),
            ("cache_objects", COUNTER),
            ("cache_payload_bytes", COUNTER),
        ],
        order_by: &[],
    },
    Table {
        name: SESSION_STATS,
        columns: &[
            ("session_id", COUNTER),
            ("peer", "LowCardinality(String)"),
            ("app_id", "LowCardinality(String)"),
            ("remote_address", "Nullable(String)"),
            ("local_ip", "Nullable(String)"),
            ("dialed_relay_id", "Nullable(String)"),
            ("rtt_us", COUNTER),
            ("current_mtu", "UInt16"),
            ("sent_bytes", COUNTER),
            ("sent_packets", COUNTER),
            ("lost_packets", COUNTER),
            ("cwnd", COUNTER),
            ("congestion_events", COUNTER),
            ("sent_stream_data_blocked", COUNTER),
            ("sent_data_blocked", COUNTER),
            ("received_stop_sending", COUNTER),
            ("received_bytes", COUNTER),
            ("received_stream_data_blocked", COUNTER),
            ("received_data_blocked", COUNTER),
            ("received_reset_stream", COUNTER),
        ],
        order_by: &["session_id"],
    },
    Table {
        name: TRACK_STATS,
        columns: &[
            ("namespace", "String"),
            ("name", "String"),
            ("publisher_session_id", COUNTER),
            ("bytes_received", COUNTER),
            ("max_arrival_gap_since_last_snapshot_us", COUNTER),
        ],
        order_by: &["publisher_session_id", "namespace", "name"],
    },
    Table {
        name: SUBSCRIPTION_STATS,
        columns: &[
            ("namespace", "String"),
            ("name", "String"),
            ("publisher_session_id", COUNTER),
            ("subscriber_session_id", COUNTER),
            ("request_id", COUNTER),
            ("bytes_sent", COUNTER),
            ("streams_reset", COUNTER),
            ("lag_behind_newest_received_us", COUNTER),
        ],
        order_by: &["subscriber_session_id", "request_id"],
    },
];

pub fn statements(database: &str) -> Vec<String> {
    std::iter::once(format!("CREATE DATABASE IF NOT EXISTS {database}"))
        .chain(TABLES.iter().map(|table| create_table(database, table)))
        .collect()
}

fn create_table(database: &str, table: &Table) -> String {
    let columns: Vec<String> = [
        "relay_id LowCardinality(String)".to_string(),
        "timestamp_ms UInt64".to_string(),
        "ts DateTime64(3, 'UTC') DEFAULT fromUnixTimestamp64Milli(toInt64(timestamp_ms))"
            .to_string(),
    ]
    .into_iter()
    .chain(
        table
            .columns
            .iter()
            .map(|(name, column_type)| format!("{name} {column_type}")),
    )
    .collect();
    let order_by: Vec<&str> = std::iter::once("relay_id")
        .chain(table.order_by.iter().copied())
        .chain(std::iter::once("timestamp_ms"))
        .collect();
    format!(
        "CREATE TABLE IF NOT EXISTS {database}.{name} ({columns}) \
         ENGINE = MergeTree \
         PARTITION BY toYYYYMMDD(ts) \
         ORDER BY ({order_by}) \
         TTL toDateTime(ts) + INTERVAL {RETENTION_DAYS} DAY \
         SETTINGS ttl_only_drop_parts = 1",
        name = table.name,
        columns = columns.join(", "),
        order_by = order_by.join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::{SESSION_STATS, statements};

    #[test]
    fn every_table_drops_whole_day_partitions_after_the_retention() {
        // Act
        let statements = statements("obs");

        // Assert
        assert_eq!(statements[0], "CREATE DATABASE IF NOT EXISTS obs");
        assert_eq!(statements.len(), 6);
        for statement in &statements[1..] {
            assert!(
                statement.contains("PARTITION BY toYYYYMMDD(ts)"),
                "{statement}"
            );
            assert!(
                statement.contains("TTL toDateTime(ts) + INTERVAL 7 DAY"),
                "{statement}"
            );
            assert!(statement.contains("ttl_only_drop_parts = 1"), "{statement}");
        }
    }

    #[test]
    fn session_rows_are_ordered_by_relay_session_and_time() {
        // Act
        let statement = statements("obs")
            .into_iter()
            .find(|statement| statement.contains(&format!("obs.{SESSION_STATS} ")))
            .unwrap();

        // Assert
        assert!(statement.contains("ORDER BY (relay_id, session_id, timestamp_ms)"));
    }
}
