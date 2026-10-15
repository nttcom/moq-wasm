use std::time::Instant;

use anyhow::Context;
use serde::Deserialize;

use crate::clickhouse::ClickHouse;

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
    /// MergeTree settings beyond the shared ones. A table created without them
    /// is rebuilt at startup, because ClickHouse cannot alter them in place.
    settings: &'static [&'static str],
}

const TABLES: &[Table] = &[
    Table {
        name: SNAPSHOTS,
        columns: &[("payload", "String")],
        order_by: &[],
        // With a few-KB payload, a granule under the default settings holds about 1500 rows,
        // so a `?at=` lookup of one row per relay decompresses thousands of payloads.
        settings: &["index_granularity = 64"],
    },
    Table {
        name: PROCESS_STATS,
        columns: &[
            ("rss_bytes", "Nullable(UInt64)"),
            ("cache_tracks", COUNTER),
            ("cache_objects", COUNTER),
            ("cache_payload_bytes", COUNTER),
            ("disk_cache_bytes", COUNTER),
            ("disk_cache_max_bytes", COUNTER),
            ("client_sessions", COUNTER),
            ("draining_since_ms", "Nullable(UInt64)"),
        ],
        order_by: &[],
        settings: &[],
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
        settings: &[],
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
        settings: &[],
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
        settings: &[],
    },
];

/// Every table starts with these, ahead of its own `columns`.
const KEY_COLUMNS: &[(&str, &str)] = &[
    ("relay_id", "LowCardinality(String)"),
    ("timestamp_ms", "UInt64"),
    (
        "ts",
        "DateTime64(3, 'UTC') DEFAULT fromUnixTimestamp64Milli(toInt64(timestamp_ms))",
    ),
];

/// Creates the database and its tables, then rebuilds every table that lacks
/// its `settings`. Call it before anything inserts: a rebuild copies the
/// table and swaps the copy in, so rows inserted during the copy are lost.
pub async fn apply(clickhouse: &ClickHouse) -> anyhow::Result<()> {
    let database = clickhouse.database();
    for statement in statements(database) {
        clickhouse.execute(&statement).await?;
    }
    for table in TABLES.iter().filter(|table| !table.settings.is_empty()) {
        // A rebuild interrupted after the swap leaves the old data here.
        clickhouse
            .execute(&format!(
                "DROP TABLE IF EXISTS {database}.{} SYNC",
                rebuild_name(table)
            ))
            .await?;
        if has_settings(&engine_of(clickhouse, table).await?, table) {
            continue;
        }
        tracing::info!(table = table.name, "rebuilding table to apply its settings");
        let started = Instant::now();
        for statement in rebuild_statements(database, table) {
            clickhouse.execute(&statement).await?;
        }
        tracing::info!(
            table = table.name,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "table rebuilt"
        );
    }
    Ok(())
}

/// Idempotent: a table created by an older build gains the columns added
/// since, so a query naming them does not fail.
pub fn statements(database: &str) -> Vec<String> {
    std::iter::once(format!("CREATE DATABASE IF NOT EXISTS {database}"))
        .chain(
            TABLES
                .iter()
                .map(|table| create_table(database, table, table.name)),
        )
        .chain(TABLES.iter().flat_map(|table| {
            table.columns.iter().map(move |(name, column_type)| {
                format!(
                    "ALTER TABLE {database}.{table} ADD COLUMN IF NOT EXISTS {name} {column_type}",
                    table = table.name
                )
            })
        }))
        .collect()
}

#[derive(Deserialize)]
struct EngineRow {
    engine_full: String,
}

async fn engine_of(clickhouse: &ClickHouse, table: &Table) -> anyhow::Result<String> {
    let rows: Vec<EngineRow> = clickhouse
        .select(
            "SELECT engine_full FROM system.tables \
             WHERE database = {database:String} AND name = {table:String}",
            &[
                ("database", clickhouse.database().to_string()),
                ("table", table.name.to_string()),
            ],
        )
        .await?;
    rows.into_iter()
        .next()
        .map(|row| row.engine_full)
        .with_context(|| format!("table {} was not created", table.name))
}

fn has_settings(engine_full: &str, table: &Table) -> bool {
    table
        .settings
        .iter()
        .all(|setting| engine_full.contains(setting))
}

fn rebuild_name(table: &Table) -> String {
    format!("{}_rebuild", table.name)
}

fn rebuild_statements(database: &str, table: &Table) -> Vec<String> {
    let name = table.name;
    let rebuild = rebuild_name(table);
    // Named, not `*`: a column added by ALTER sits last in the old table.
    let columns = KEY_COLUMNS
        .iter()
        .chain(table.columns)
        .map(|(column, _)| *column)
        .collect::<Vec<_>>()
        .join(", ");
    vec![
        create_table(database, table, &rebuild),
        // Writes the inserts the previous process left queued into the old table.
        "SYSTEM FLUSH ASYNC INSERT QUEUE".to_string(),
        format!(
            "INSERT INTO {database}.{rebuild} ({columns}) SELECT {columns} FROM {database}.{name}"
        ),
        format!("EXCHANGE TABLES {database}.{name} AND {database}.{rebuild}"),
        format!("DROP TABLE {database}.{rebuild} SYNC"),
    ]
}

fn create_table(database: &str, table: &Table, name: &str) -> String {
    let columns: Vec<String> = KEY_COLUMNS
        .iter()
        .chain(table.columns)
        .map(|(column, column_type)| format!("{column} {column_type}"))
        .collect();
    let order_by: Vec<&str> = std::iter::once("relay_id")
        .chain(table.order_by.iter().copied())
        .chain(std::iter::once("timestamp_ms"))
        .collect();
    let settings: Vec<&str> = std::iter::once("ttl_only_drop_parts = 1")
        .chain(table.settings.iter().copied())
        .collect();
    format!(
        "CREATE TABLE IF NOT EXISTS {database}.{name} ({columns}) \
         ENGINE = MergeTree \
         PARTITION BY toYYYYMMDD(ts) \
         ORDER BY ({order_by}) \
         TTL toDateTime(ts) + INTERVAL {RETENTION_DAYS} DAY \
         SETTINGS {settings}",
        columns = columns.join(", "),
        order_by = order_by.join(", "),
        settings = settings.join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        PROCESS_STATS, SESSION_STATS, SNAPSHOTS, TABLES, Table, has_settings, rebuild_statements,
        statements,
    };

    fn table(name: &str) -> &'static Table {
        TABLES.iter().find(|table| table.name == name).unwrap()
    }

    #[test]
    fn every_table_drops_whole_day_partitions_after_the_retention() {
        // Act
        let statements = statements("obs");

        // Assert
        assert_eq!(statements[0], "CREATE DATABASE IF NOT EXISTS obs");
        for statement in statements
            .iter()
            .filter(|statement| statement.starts_with("CREATE TABLE"))
        {
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
    fn a_table_from_an_older_build_gains_the_new_columns() {
        // Act
        let statements = statements("obs");

        // Assert
        assert!(
            statements.contains(
                &"ALTER TABLE obs.process_stats ADD COLUMN IF NOT EXISTS disk_cache_bytes UInt64"
                    .to_string()
            )
        );
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

    #[test]
    fn a_snapshots_table_without_the_fine_granularity_needs_a_rebuild() {
        // Arrange
        let old = "MergeTree PARTITION BY toYYYYMMDD(ts) ORDER BY (relay_id, timestamp_ms) \
                   SETTINGS ttl_only_drop_parts = 1, index_granularity = 8192";
        let new = "MergeTree PARTITION BY toYYYYMMDD(ts) ORDER BY (relay_id, timestamp_ms) \
                   SETTINGS ttl_only_drop_parts = 1, index_granularity = 64";

        // Act / Assert
        assert!(!has_settings(old, table(SNAPSHOTS)));
        assert!(has_settings(new, table(SNAPSHOTS)));
    }

    #[test]
    fn a_new_snapshots_table_is_created_with_the_fine_granularity() {
        // Act
        let statements = statements("obs");

        // Assert
        let create = statements
            .iter()
            .find(|statement| statement.contains(&format!("obs.{SNAPSHOTS} ")))
            .unwrap();
        assert!(create.ends_with("SETTINGS ttl_only_drop_parts = 1, index_granularity = 64"));
    }

    #[test]
    fn a_rebuild_copies_named_columns_then_swaps_and_drops_the_old_table() {
        // Act
        let statements = rebuild_statements("obs", table(PROCESS_STATS));

        // Assert
        assert!(
            statements[0].starts_with("CREATE TABLE IF NOT EXISTS obs.process_stats_rebuild (")
        );
        assert_eq!(statements[1], "SYSTEM FLUSH ASYNC INSERT QUEUE");
        assert!(statements[2].starts_with(
            "INSERT INTO obs.process_stats_rebuild (relay_id, timestamp_ms, ts, rss_bytes, "
        ));
        assert!(statements[2].ends_with(" FROM obs.process_stats"));
        assert_eq!(
            &statements[3..],
            [
                "EXCHANGE TABLES obs.process_stats AND obs.process_stats_rebuild",
                "DROP TABLE obs.process_stats_rebuild SYNC",
            ]
        );
    }
}
