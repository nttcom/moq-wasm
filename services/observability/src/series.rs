use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::{
    clickhouse::ClickHouse,
    schema::{SESSION_STATS, SUBSCRIPTION_STATS, TRACK_STATS},
};

const MIN_BUCKET_MS: u64 = 1_000;
const BITS_PER_BYTE: f64 = 8.0;
const MEGA: f64 = 1_000_000.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeriesTarget {
    /// The app's client sessions on the relay, summed.
    Relay,
    Session {
        session_id: u64,
    },
    Track {
        publisher_session_id: u64,
        namespace: String,
        name: String,
    },
    Subscription {
        subscriber_session_id: u64,
        request_id: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesQuery {
    /// Only this app's sessions and namespaces are read.
    pub app_id: String,
    pub relay_id: String,
    pub target: SeriesTarget,
    pub from_ms: u64,
    pub to_ms: u64,
    pub points: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Series {
    pub t: Vec<u64>,
    pub series: BTreeMap<&'static str, Vec<Option<f64>>>,
}

#[derive(Clone, Copy)]
enum Metric {
    /// Per-second rate of the summed counter columns, times the scale.
    Rate(&'static [&'static str], f64),
    /// Rate of the numerator counter over the rate of the denominator counter,
    /// times the scale.
    RateRatio(&'static str, &'static str, f64),
    /// Mean of the gauge column, times the scale.
    Mean(&'static str, f64),
    /// Maximum of the gauge column, times the scale.
    Max(&'static str, f64),
    Entities,
}

impl Metric {
    fn counters(self) -> Vec<&'static str> {
        match self {
            Self::Rate(columns, _) => columns.to_vec(),
            Self::RateRatio(numerator, denominator, _) => vec![numerator, denominator],
            Self::Mean(..) | Self::Max(..) | Self::Entities => vec![],
        }
    }
}

const MBPS: f64 = BITS_PER_BYTE / MEGA;

const SESSION_METRICS: &[(&str, Metric)] = &[
    ("sent_mbps", Metric::Rate(&["sent_bytes"], MBPS)),
    ("received_mbps", Metric::Rate(&["received_bytes"], MBPS)),
    ("rtt_ms", Metric::Mean("rtt_us", 1e-3)),
    ("cwnd_kb", Metric::Mean("cwnd", 1.0 / 1024.0)),
    (
        "loss_percent",
        Metric::RateRatio("lost_packets", "sent_packets", 100.0),
    ),
    (
        "congestion_per_s",
        Metric::Rate(&["congestion_events"], 1.0),
    ),
    (
        "relay_blocked_per_s",
        Metric::Rate(&["sent_stream_data_blocked", "sent_data_blocked"], 1.0),
    ),
    (
        "peer_blocked_per_s",
        Metric::Rate(
            &["received_stream_data_blocked", "received_data_blocked"],
            1.0,
        ),
    ),
    (
        "peer_resets_per_s",
        Metric::Rate(&["received_reset_stream"], 1.0),
    ),
    (
        "stop_sending_per_s",
        Metric::Rate(&["received_stop_sending"], 1.0),
    ),
];

const RELAY_METRICS: &[(&str, Metric)] = &[
    ("ingress_mbps", Metric::Rate(&["received_bytes"], MBPS)),
    ("egress_mbps", Metric::Rate(&["sent_bytes"], MBPS)),
    (
        "egress_loss_percent",
        Metric::RateRatio("lost_packets", "sent_packets", 100.0),
    ),
    (
        "congestion_per_s",
        Metric::Rate(&["congestion_events"], 1.0),
    ),
    (
        "relay_blocked_per_s",
        Metric::Rate(&["sent_stream_data_blocked", "sent_data_blocked"], 1.0),
    ),
    (
        "peers_blocked_per_s",
        Metric::Rate(
            &["received_stream_data_blocked", "received_data_blocked"],
            1.0,
        ),
    ),
    (
        "peer_resets_per_s",
        Metric::Rate(&["received_reset_stream"], 1.0),
    ),
    ("sessions", Metric::Entities),
];

const TRACK_METRICS: &[(&str, Metric)] = &[
    ("received_mbps", Metric::Rate(&["bytes_received"], MBPS)),
    (
        "max_arrival_gap_ms",
        Metric::Max("max_arrival_gap_since_last_snapshot_us", 1e-3),
    ),
];

const SUBSCRIPTION_METRICS: &[(&str, Metric)] = &[
    ("sent_mbps", Metric::Rate(&["bytes_sent"], MBPS)),
    ("resets_per_s", Metric::Rate(&["streams_reset"], 1.0)),
    ("lag_ms", Metric::Max("lag_behind_newest_received_us", 1e-3)),
];

struct Plan {
    table: &'static str,
    filters: Vec<(&'static str, &'static str, String)>,
    condition: &'static str,
    entity: Option<&'static str>,
    metrics: &'static [(&'static str, Metric)],
}

/// Relay sessions are left out: they carry every app's traffic.
pub(crate) const APP_CLIENT_SESSION: &str = "peer = 'client' AND app_id = {app:String}";
pub(crate) const APP_NAMESPACE: &str =
    "(namespace = {app:String} OR startsWith(namespace, concat({app:String}, '/')))";

fn plan(target: &SeriesTarget) -> Plan {
    match target {
        SeriesTarget::Relay => Plan {
            table: SESSION_STATS,
            filters: vec![],
            condition: APP_CLIENT_SESSION,
            entity: Some("session_id"),
            metrics: RELAY_METRICS,
        },
        SeriesTarget::Session { session_id } => Plan {
            table: SESSION_STATS,
            filters: vec![("session_id", "UInt64", session_id.to_string())],
            condition: APP_CLIENT_SESSION,
            entity: None,
            metrics: SESSION_METRICS,
        },
        SeriesTarget::Track {
            publisher_session_id,
            namespace,
            name,
        } => Plan {
            table: TRACK_STATS,
            filters: vec![
                (
                    "publisher_session_id",
                    "UInt64",
                    publisher_session_id.to_string(),
                ),
                ("namespace", "String", namespace.clone()),
                ("name", "String", name.clone()),
            ],
            condition: APP_NAMESPACE,
            entity: None,
            metrics: TRACK_METRICS,
        },
        SeriesTarget::Subscription {
            subscriber_session_id,
            request_id,
        } => Plan {
            table: SUBSCRIPTION_STATS,
            filters: vec![
                (
                    "subscriber_session_id",
                    "UInt64",
                    subscriber_session_id.to_string(),
                ),
                ("request_id", "UInt64", request_id.to_string()),
            ],
            condition: APP_NAMESPACE,
            entity: None,
            metrics: SUBSCRIPTION_METRICS,
        },
    }
}

pub fn bucket_ms(query: &SeriesQuery) -> u64 {
    let span = query.to_ms.saturating_sub(query.from_ms);
    (span / query.points.max(1)).max(MIN_BUCKET_MS)
}

fn select_sql(database: &str, plan: &Plan) -> String {
    let mut columns: BTreeSet<String> =
        BTreeSet::from(["max(timestamp_ms) AS sampled_ms".to_string()]);
    for (_, metric) in plan.metrics {
        for counter in metric.counters() {
            columns.insert(format!("max({counter}) AS {counter}"));
        }
        match metric {
            Metric::Mean(column, _) => {
                columns.insert(format!("avg({column}) AS {column}"));
            }
            Metric::Max(column, _) => {
                columns.insert(format!("max({column}) AS {column}"));
            }
            _ => {}
        }
    }
    let entity = plan
        .entity
        .map_or(String::new(), |entity| format!(", {entity} AS entity"));
    let group_entity = plan
        .entity
        .map_or(String::new(), |entity| format!(", {entity}"));
    let filters: String = plan
        .filters
        .iter()
        .map(|(column, column_type, _)| format!(" AND {column} = {{{column}:{column_type}}}"))
        .collect();
    format!(
        "SELECT intDiv(timestamp_ms, {{bucket:UInt64}}) * {{bucket:UInt64}} AS t{entity}, {columns} \
         FROM {database}.{table} \
         WHERE relay_id = {{relay_id:String}} \
         AND timestamp_ms >= {{from:UInt64}} AND timestamp_ms <= {{to:UInt64}}{filters} AND {condition} \
         GROUP BY t{group_entity} ORDER BY t",
        columns = columns.into_iter().collect::<Vec<_>>().join(", "),
        table = plan.table,
        condition = plan.condition,
    )
}

fn number(row: &Map<String, Value>, column: &str) -> Option<f64> {
    row.get(column).and_then(Value::as_f64)
}

fn bucket_of(row: &Map<String, Value>) -> Option<u64> {
    row.get("t").and_then(Value::as_u64)
}

fn entity_of(row: &Map<String, Value>) -> u64 {
    row.get("entity").and_then(Value::as_u64).unwrap_or(0)
}

fn sampled_at(row: &Map<String, Value>) -> Option<u64> {
    row.get("sampled_ms")
        .and_then(Value::as_u64)
        .or_else(|| bucket_of(row))
}

fn rate(
    previous: &Map<String, Value>,
    current: &Map<String, Value>,
    columns: &[&str],
) -> Option<f64> {
    let elapsed_s = (sampled_at(current)? as f64 - sampled_at(previous)? as f64) / 1000.0;
    if elapsed_s <= 0.0 {
        return None;
    }
    let delta: f64 = columns
        .iter()
        .map(|column| Some(number(current, column)? - number(previous, column)?))
        .sum::<Option<f64>>()?;
    (delta >= 0.0).then_some(delta / elapsed_s)
}

fn compute(rows: &[Map<String, Value>], metrics: &[(&'static str, Metric)]) -> Series {
    let mut by_entity: BTreeMap<u64, Vec<&Map<String, Value>>> = BTreeMap::new();
    for row in rows {
        by_entity.entry(entity_of(row)).or_default().push(row);
    }
    let buckets: BTreeSet<u64> = rows.iter().filter_map(bucket_of).collect();
    let t: Vec<u64> = buckets.iter().copied().collect();
    let index: BTreeMap<u64, usize> = t
        .iter()
        .enumerate()
        .map(|(i, bucket)| (*bucket, i))
        .collect();
    let mut series = BTreeMap::new();
    for (name, metric) in metrics {
        let mut values: Vec<Option<f64>> = vec![None; t.len()];
        let mut ratio_parts: Vec<Option<(f64, f64)>> = vec![None; t.len()];
        for entity_rows in by_entity.values() {
            for (position, row) in entity_rows.iter().enumerate() {
                let Some(&slot) = bucket_of(row).and_then(|bucket| index.get(&bucket)) else {
                    continue;
                };
                let previous = position.checked_sub(1).map(|p| entity_rows[p]);
                let value = match *metric {
                    Metric::Rate(columns, scale) => previous
                        .and_then(|previous| rate(previous, row, columns))
                        .map(|v| v * scale),
                    Metric::RateRatio(numerator, denominator, _) => {
                        if let Some(previous) = previous
                            && let (Some(top), Some(bottom)) = (
                                rate(previous, row, &[numerator]),
                                rate(previous, row, &[denominator]),
                            )
                        {
                            let (sum_top, sum_bottom) = ratio_parts[slot].unwrap_or((0.0, 0.0));
                            ratio_parts[slot] = Some((sum_top + top, sum_bottom + bottom));
                        }
                        None
                    }
                    Metric::Mean(column, scale) | Metric::Max(column, scale) => {
                        number(row, column).map(|v| v * scale)
                    }
                    Metric::Entities => Some(1.0),
                };
                if let Some(value) = value {
                    values[slot] = Some(values[slot].unwrap_or(0.0) + value);
                }
            }
        }
        if let Metric::RateRatio(_, _, scale) = metric {
            values = ratio_parts
                .iter()
                .map(|parts| {
                    parts.map(|(top, bottom)| {
                        if bottom > 0.0 {
                            top / bottom * scale
                        } else {
                            0.0
                        }
                    })
                })
                .collect();
        }
        series.insert(*name, values);
    }
    Series { t, series }
}

pub async fn query(clickhouse: &ClickHouse, query: &SeriesQuery) -> anyhow::Result<Series> {
    let plan = plan(&query.target);
    let mut params = vec![
        ("bucket", bucket_ms(query).to_string()),
        ("app", query.app_id.clone()),
        ("relay_id", query.relay_id.clone()),
        ("from", query.from_ms.to_string()),
        ("to", query.to_ms.to_string()),
    ];
    params.extend(
        plan.filters
            .iter()
            .map(|(column, _, value)| (*column, value.clone())),
    );
    let rows: Vec<Map<String, Value>> = clickhouse
        .select(&select_sql(clickhouse.database(), &plan), &params)
        .await?;
    Ok(compute(&rows, plan.metrics))
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::{
        APP_CLIENT_SESSION, APP_NAMESPACE, Metric, SeriesQuery, SeriesTarget, bucket_ms, compute,
        plan, select_sql,
    };

    fn row(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn every_target_reads_only_the_apps_client_sessions_or_namespaces() {
        // Arrange
        let targets = [
            SeriesTarget::Relay,
            SeriesTarget::Session { session_id: 1 },
            SeriesTarget::Track {
                publisher_session_id: 1,
                namespace: "app/room".to_string(),
                name: "video".to_string(),
            },
            SeriesTarget::Subscription {
                subscriber_session_id: 1,
                request_id: 2,
            },
        ];

        // Act
        let sqls: Vec<String> = targets
            .iter()
            .map(|target| select_sql("obs", &plan(target)))
            .collect();

        // Assert
        let wanted = [
            APP_CLIENT_SESSION,
            APP_CLIENT_SESSION,
            APP_NAMESPACE,
            APP_NAMESPACE,
        ];
        for (sql, wanted) in sqls.iter().zip(wanted) {
            assert!(sql.contains(wanted), "{sql}");
        }
    }

    #[test]
    fn counters_become_per_second_rates_between_buckets() {
        // Arrange
        let rows = vec![
            row(json!({ "t": 0, "sent_bytes": 1_000 })),
            row(json!({ "t": 2_000, "sent_bytes": 5_000 })),
        ];
        let metrics = [("sent", Metric::Rate(&["sent_bytes"], 1.0))];

        // Act
        let series = compute(&rows, &metrics);

        // Assert
        assert_eq!(series.t, vec![0, 2_000]);
        assert_eq!(series.series["sent"], vec![None, Some(2_000.0)]);
    }

    #[test]
    fn a_partly_filled_bucket_is_divided_by_the_time_it_actually_covers() {
        // Arrange
        let rows = vec![
            row(json!({ "t": 0, "sampled_ms": 39_000, "sent_bytes": 39_000 })),
            row(json!({ "t": 40_000, "sampled_ms": 44_000, "sent_bytes": 44_000 })),
        ];
        let metrics = [("sent", Metric::Rate(&["sent_bytes"], 1.0))];

        // Act / Assert
        assert_eq!(
            compute(&rows, &metrics).series["sent"],
            vec![None, Some(1_000.0)]
        );
    }

    #[test]
    fn a_counter_that_went_backwards_has_no_rate() {
        // Arrange
        let rows = vec![
            row(json!({ "t": 0, "sent_bytes": 5_000 })),
            row(json!({ "t": 1_000, "sent_bytes": 10 })),
        ];
        let metrics = [("sent", Metric::Rate(&["sent_bytes"], 1.0))];

        // Act / Assert
        assert_eq!(compute(&rows, &metrics).series["sent"], vec![None, None]);
    }

    #[test]
    fn entity_rates_are_summed_and_ratios_weighted_by_their_denominators() {
        // Arrange
        let rows = vec![
            row(json!({ "t": 0, "entity": 1, "lost": 0, "sent": 0 })),
            row(json!({ "t": 0, "entity": 2, "lost": 0, "sent": 0 })),
            row(json!({ "t": 1_000, "entity": 1, "lost": 10, "sent": 100 })),
            row(json!({ "t": 1_000, "entity": 2, "lost": 0, "sent": 900 })),
        ];
        let metrics = [
            ("sent", Metric::Rate(&["sent"], 1.0)),
            ("loss", Metric::RateRatio("lost", "sent", 100.0)),
            ("entities", Metric::Entities),
        ];

        // Act
        let series = compute(&rows, &metrics);

        // Assert
        assert_eq!(series.series["sent"], vec![None, Some(1_000.0)]);
        assert_eq!(series.series["loss"], vec![None, Some(1.0)]);
        assert_eq!(series.series["entities"], vec![Some(2.0), Some(2.0)]);
    }

    #[test]
    fn gauges_are_scaled_per_bucket() {
        // Arrange
        let rows = vec![row(json!({ "t": 0, "rtt_us": 12_000.0 }))];
        let metrics = [("rtt_ms", Metric::Mean("rtt_us", 1e-3))];

        // Act / Assert
        assert_eq!(compute(&rows, &metrics).series["rtt_ms"], vec![Some(12.0)]);
    }

    #[test]
    fn buckets_split_the_range_into_the_requested_points_but_never_below_a_second() {
        // Arrange
        let query = |span: u64| SeriesQuery {
            app_id: "app".to_string(),
            relay_id: "relay-a".to_string(),
            target: SeriesTarget::Relay,
            from_ms: 0,
            to_ms: span,
            points: 90,
        };

        // Act / Assert
        assert_eq!(bucket_ms(&query(3_600_000)), 40_000);
        assert_eq!(bucket_ms(&query(30_000)), 1_000);
    }
}
