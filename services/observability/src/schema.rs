use std::time::{Duration, Instant};

use anyhow::Context;
use serde::Deserialize;
use telemetry::relay_stats::{EVENT_ATTRIBUTE, EVENT_NAME};

use crate::clickhouse::ClickHouse;

pub const RETENTION_DAYS: u32 = 7;

pub const SNAPSHOTS: &str = "snapshots";
pub const PROCESS_STATS: &str = "process_stats";
pub const SESSION_STATS: &str = "session_stats";
pub const TRACK_STATS: &str = "track_stats";
pub const SUBSCRIPTION_STATS: &str = "subscription_stats";

/// The table the ClickStack collector writes the relays' OTLP logs into.
const OTEL_LOGS: &str = "otel.otel_logs";
const OTEL_LOGS_POLL_INTERVAL: Duration = Duration::from_secs(5);

const COUNTER: &str = "UInt64";

/// Where a table's rows sit in a snapshot.
enum Rows {
    /// The whole snapshot JSON, as `payload`.
    Snapshot,
    /// One row from the object under this key.
    Object(&'static str),
    /// One row per element of the array under this key.
    Array(&'static str),
}

struct Table {
    name: &'static str,
    rows: Rows,
    columns: &'static [(&'static str, &'static str)],
    order_by: &'static [&'static str],
    /// MergeTree settings beyond the shared ones. A table created without them
    /// is rebuilt at startup, because ClickHouse cannot alter them in place.
    settings: &'static [&'static str],
}

const TABLES: &[Table] = &[
    Table {
        name: SNAPSHOTS,
        rows: Rows::Snapshot,
        columns: &[("payload", "String")],
        order_by: &[],
        // With a few-KB payload, a granule under the default settings holds about 1500 rows,
        // so a `?at=` lookup of one row per relay decompresses thousands of payloads.
        settings: &["index_granularity = 64"],
    },
    Table {
        name: PROCESS_STATS,
        rows: Rows::Object("process"),
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
        rows: Rows::Array("sessions"),
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
        rows: Rows::Array("tracks"),
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
        rows: Rows::Array("subscriptions"),
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
/// its `settings`. Once the collector has created `otel.otel_logs`, it creates
/// the materialized views that fill the tables from the relays' snapshot
/// records.
/// The views are dropped first, so a rebuild never copies a table something
/// inserts into. Snapshots logged before the views are recreated are not stored.
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
    wait_for_otel_logs(clickhouse).await?;
    for table in TABLES {
        clickhouse.execute(&create_view(database, table)).await?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct ExistsRow {
    result: u8,
}

async fn wait_for_otel_logs(clickhouse: &ClickHouse) -> anyhow::Result<()> {
    let mut logged = false;
    loop {
        let rows: Vec<ExistsRow> = clickhouse
            .select(&format!("EXISTS TABLE {OTEL_LOGS}"), &[])
            .await?;
        if rows.first().is_some_and(|row| row.result == 1) {
            return Ok(());
        }
        if !logged {
            tracing::info!(
                table = OTEL_LOGS,
                "waiting for the collector to create its table"
            );
            logged = true;
        }
        tokio::time::sleep(OTEL_LOGS_POLL_INTERVAL).await;
    }
}

/// Idempotent: a table created by an older build gains the columns added
/// since, so a query naming them does not fail.
pub fn statements(database: &str) -> Vec<String> {
    std::iter::once(format!("CREATE DATABASE IF NOT EXISTS {database}"))
        .chain(TABLES.iter().map(|table| {
            format!(
                "DROP VIEW IF EXISTS {database}.{view}",
                view = view_name(table)
            )
        }))
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
        format!(
            "INSERT INTO {database}.{rebuild} ({columns}) SELECT {columns} FROM {database}.{name}"
        ),
        format!("EXCHANGE TABLES {database}.{name} AND {database}.{rebuild}"),
        format!("DROP TABLE {database}.{rebuild} SYNC"),
    ]
}

fn view_name(table: &Table) -> String {
    format!("{}_from_otel", table.name)
}

/// A materialized view from `otel.otel_logs` into `table`: ClickHouse runs it
/// on every block the collector inserts, so each snapshot record becomes the
/// table's rows as it arrives.
fn create_view(database: &str, table: &Table) -> String {
    let extract = |source: &str| {
        table
            .columns
            .iter()
            .map(|(column, column_type)| {
                format!("JSONExtract({source}, '{column}', '{column_type}') AS {column}")
            })
            .collect::<Vec<_>>()
    };
    let (columns, array_join) = match table.rows {
        Rows::Snapshot => (vec!["snapshot AS payload".to_string()], String::new()),
        Rows::Object(key) => (
            extract(&format!("JSONExtractRaw(snapshot, '{key}')")),
            String::new(),
        ),
        Rows::Array(key) => (
            extract("item"),
            format!(" ARRAY JOIN JSONExtractArrayRaw(snapshot, '{key}') AS item"),
        ),
    };
    format!(
        "CREATE MATERIALIZED VIEW {database}.{view} TO {database}.{name} AS \
         SELECT JSONExtractString(snapshot, 'relay_id') AS relay_id, \
         JSONExtractUInt(snapshot, 'timestamp_ms') AS timestamp_ms, {columns} \
         FROM (SELECT LogAttributes['{EVENT_ATTRIBUTE}'] AS snapshot FROM {OTEL_LOGS} \
         WHERE EventName = '{EVENT_NAME}'){array_join}",
        view = view_name(table),
        name = table.name,
        columns = columns.join(", "),
    )
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
        PROCESS_STATS, SESSION_STATS, SNAPSHOTS, TABLES, Table, create_view, has_settings,
        rebuild_statements, statements,
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
        assert!(statements[1].starts_with(
            "INSERT INTO obs.process_stats_rebuild (relay_id, timestamp_ms, ts, rss_bytes, "
        ));
        assert!(statements[1].ends_with(" FROM obs.process_stats"));
        assert_eq!(
            &statements[2..],
            [
                "EXCHANGE TABLES obs.process_stats AND obs.process_stats_rebuild",
                "DROP TABLE obs.process_stats_rebuild SYNC",
            ]
        );
    }

    #[test]
    fn every_view_is_dropped_before_the_tables_are_touched() {
        // Act
        let statements = statements("obs");

        // Assert
        assert_eq!(
            &statements[1..=TABLES.len()],
            TABLES
                .iter()
                .map(|table| format!("DROP VIEW IF EXISTS obs.{}_from_otel", table.name))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_view_reads_the_snapshot_attribute_of_the_relays_snapshot_records() {
        // Act
        let view = create_view("obs", table(SNAPSHOTS));

        // Assert
        assert_eq!(
            view,
            "CREATE MATERIALIZED VIEW obs.snapshots_from_otel TO obs.snapshots AS \
             SELECT JSONExtractString(snapshot, 'relay_id') AS relay_id, \
             JSONExtractUInt(snapshot, 'timestamp_ms') AS timestamp_ms, snapshot AS payload \
             FROM (SELECT LogAttributes['snapshot'] AS snapshot FROM otel.otel_logs \
             WHERE EventName = 'relay.stats.snapshot')"
        );
    }

    #[test]
    fn a_view_extracts_each_column_from_the_object_or_array_element_of_its_table() {
        // Act
        let process = create_view("obs", table(PROCESS_STATS));
        let sessions = create_view("obs", table(SESSION_STATS));

        // Assert
        assert!(process.contains(
            "JSONExtract(JSONExtractRaw(snapshot, 'process'), 'rss_bytes', 'Nullable(UInt64)') AS rss_bytes"
        ));
        assert!(!process.contains("ARRAY JOIN"));
        assert!(sessions.contains("JSONExtract(item, 'peer', 'LowCardinality(String)') AS peer"));
        assert!(
            sessions.ends_with(" ARRAY JOIN JSONExtractArrayRaw(snapshot, 'sessions') AS item")
        );
    }
}
