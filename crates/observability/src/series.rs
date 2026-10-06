use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::{
    clickhouse::ClickHouse,
    schema::{PROCESS_STATS, SESSION_STATS, SUBSCRIPTION_STATS, TRACK_STATS},
};

const MIN_BUCKET_MS: u64 = 1_000;
const BITS_PER_BYTE: f64 = 8.0;
const MEGA: f64 = 1_000_000.0;
const MEBIBYTE: f64 = 1024.0 * 1024.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeriesTarget {
    Process,
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
    Rate {
        columns: &'static [&'static str],
        scale: f64,
    },
    RateRatio {
        numerator: &'static str,
        denominator: &'static str,
        scale: f64,
    },
    Mean {
        column: &'static str,
        scale: f64,
    },
    Max {
        column: &'static str,
        scale: f64,
    },
    Entities,
}

impl Metric {
    fn counters(self) -> Vec<&'static str> {
        match self {
            Self::Rate { columns, .. } => columns.to_vec(),
            Self::RateRatio {
                numerator,
                denominator,
                ..
            } => vec![numerator, denominator],
            Self::Mean { .. } | Self::Max { .. } | Self::Entities => vec![],
        }
    }
}

const MBPS: f64 = BITS_PER_BYTE / MEGA;

const SESSION_METRICS: &[(&str, Metric)] = &[
    (
        "sent_mbps",
        Metric::Rate {
            columns: &["sent_bytes"],
            scale: MBPS,
        },
    ),
    (
        "received_mbps",
        Metric::Rate {
            columns: &["received_bytes"],
            scale: MBPS,
        },
    ),
    (
        "rtt_ms",
        Metric::Mean {
            column: "rtt_us",
            scale: 1e-3,
        },
    ),
    (
        "cwnd_kb",
        Metric::Mean {
            column: "cwnd",
            scale: 1.0 / 1024.0,
        },
    ),
    (
        "loss_percent",
        Metric::RateRatio {
            numerator: "lost_packets",
            denominator: "sent_packets",
            scale: 100.0,
        },
    ),
    (
        "congestion_per_s",
        Metric::Rate {
            columns: &["congestion_events"],
            scale: 1.0,
        },
    ),
    (
        "relay_blocked_per_s",
        Metric::Rate {
            columns: &["sent_stream_data_blocked", "sent_data_blocked"],
            scale: 1.0,
        },
    ),
    (
        "peer_blocked_per_s",
        Metric::Rate {
            columns: &["received_stream_data_blocked", "received_data_blocked"],
            scale: 1.0,
        },
    ),
    (
        "peer_resets_per_s",
        Metric::Rate {
            columns: &["received_reset_stream"],
            scale: 1.0,
        },
    ),
    (
        "stop_sending_per_s",
        Metric::Rate {
            columns: &["received_stop_sending"],
            scale: 1.0,
        },
    ),
];

const RELAY_METRICS: &[(&str, Metric)] = &[
    (
        "ingress_mbps",
        Metric::Rate {
            columns: &["received_bytes"],
            scale: MBPS,
        },
    ),
    (
        "egress_mbps",
        Metric::Rate {
            columns: &["sent_bytes"],
            scale: MBPS,
        },
    ),
    (
        "egress_loss_percent",
        Metric::RateRatio {
            numerator: "lost_packets",
            denominator: "sent_packets",
            scale: 100.0,
        },
    ),
    (
        "congestion_per_s",
        Metric::Rate {
            columns: &["congestion_events"],
            scale: 1.0,
        },
    ),
    (
        "relay_blocked_per_s",
        Metric::Rate {
            columns: &["sent_stream_data_blocked", "sent_data_blocked"],
            scale: 1.0,
        },
    ),
    (
        "peers_blocked_per_s",
        Metric::Rate {
            columns: &["received_stream_data_blocked", "received_data_blocked"],
            scale: 1.0,
        },
    ),
    (
        "peer_resets_per_s",
        Metric::Rate {
            columns: &["received_reset_stream"],
            scale: 1.0,
        },
    ),
    ("sessions", Metric::Entities),
];

const TRACK_METRICS: &[(&str, Metric)] = &[
    (
        "received_mbps",
        Metric::Rate {
            columns: &["bytes_received"],
            scale: MBPS,
        },
    ),
    (
        "max_arrival_gap_ms",
        Metric::Max {
            column: "max_arrival_gap_since_last_snapshot_us",
            scale: 1e-3,
        },
    ),
];

const SUBSCRIPTION_METRICS: &[(&str, Metric)] = &[
    (
        "sent_mbps",
        Metric::Rate {
            columns: &["bytes_sent"],
            scale: MBPS,
        },
    ),
    (
        "resets_per_s",
        Metric::Rate {
            columns: &["streams_reset"],
            scale: 1.0,
        },
    ),
    (
        "lag_ms",
        Metric::Max {
            column: "lag_behind_newest_received_us",
            scale: 1e-3,
        },
    ),
];

const PROCESS_METRICS: &[(&str, Metric)] = &[
    (
        "rss_mb",
        Metric::Max {
            column: "rss_bytes",
            scale: 1.0 / MEBIBYTE,
        },
    ),
    (
        "cache_mb",
        Metric::Max {
            column: "cache_payload_bytes",
            scale: 1.0 / MEBIBYTE,
        },
    ),
    (
        "cache_objects",
        Metric::Max {
            column: "cache_objects",
            scale: 1.0,
        },
    ),
];

struct Plan {
    table: &'static str,
    filters: Vec<(&'static str, &'static str, String)>,
    condition: &'static str,
    entity: Option<&'static str>,
    metrics: &'static [(&'static str, Metric)],
}

// The relay's own stats sessions carry the snapshots, not client or relay traffic.
const TRAFFIC_PEERS: &str = " AND peer IN ('client', 'relay')";

fn plan(target: &SeriesTarget) -> Plan {
    match target {
        SeriesTarget::Process => Plan {
            table: PROCESS_STATS,
            filters: vec![],
            condition: "",
            entity: None,
            metrics: PROCESS_METRICS,
        },
        SeriesTarget::Relay => Plan {
            table: SESSION_STATS,
            filters: vec![],
            condition: TRAFFIC_PEERS,
            entity: Some("session_id"),
            metrics: RELAY_METRICS,
        },
        SeriesTarget::Session { session_id } => Plan {
            table: SESSION_STATS,
            filters: vec![("session_id", "UInt64", session_id.to_string())],
            condition: "",
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
            condition: "",
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
            condition: "",
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
            Metric::Mean { column, .. } => {
                columns.insert(format!("avg({column}) AS {column}"));
            }
            Metric::Max { column, .. } => {
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
         AND timestamp_ms >= {{from:UInt64}} AND timestamp_ms <= {{to:UInt64}}{filters}{condition} \
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
                    Metric::Rate { columns, scale } => previous
                        .and_then(|previous| rate(previous, row, columns))
                        .map(|v| v * scale),
                    Metric::RateRatio {
                        numerator,
                        denominator,
                        ..
                    } => {
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
                    Metric::Mean { column, scale } | Metric::Max { column, scale } => {
                        number(row, column).map(|v| v * scale)
                    }
                    Metric::Entities => Some(1.0),
                };
                if let Some(value) = value {
                    values[slot] = Some(values[slot].unwrap_or(0.0) + value);
                }
            }
        }
        if let Metric::RateRatio { scale, .. } = metric {
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

    use super::{Metric, SeriesQuery, SeriesTarget, bucket_ms, compute, plan, select_sql};

    fn row(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn relay_totals_leave_out_the_relays_own_stats_sessions() {
        // Act
        let sql = select_sql("obs", &plan(&SeriesTarget::Relay));

        // Assert
        assert!(sql.contains("AND peer IN ('client', 'relay')"), "{sql}");
    }

    #[test]
    fn counters_become_per_second_rates_between_buckets() {
        // Arrange
        let rows = vec![
            row(json!({ "t": 0, "sent_bytes": 1_000 })),
            row(json!({ "t": 2_000, "sent_bytes": 5_000 })),
        ];
        let metrics = [(
            "sent",
            Metric::Rate {
                columns: &["sent_bytes"],
                scale: 1.0,
            },
        )];

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
        let metrics = [(
            "sent",
            Metric::Rate {
                columns: &["sent_bytes"],
                scale: 1.0,
            },
        )];

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
        let metrics = [(
            "sent",
            Metric::Rate {
                columns: &["sent_bytes"],
                scale: 1.0,
            },
        )];

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
            (
                "sent",
                Metric::Rate {
                    columns: &["sent"],
                    scale: 1.0,
                },
            ),
            (
                "loss",
                Metric::RateRatio {
                    numerator: "lost",
                    denominator: "sent",
                    scale: 100.0,
                },
            ),
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
        let metrics = [(
            "rtt_ms",
            Metric::Mean {
                column: "rtt_us",
                scale: 1e-3,
            },
        )];

        // Act / Assert
        assert_eq!(compute(&rows, &metrics).series["rtt_ms"], vec![Some(12.0)]);
    }

    #[test]
    fn buckets_split_the_range_into_the_requested_points_but_never_below_a_second() {
        // Arrange
        let query = |span: u64| SeriesQuery {
            relay_id: "relay-a".to_string(),
            target: SeriesTarget::Process,
            from_ms: 0,
            to_ms: span,
            points: 90,
        };

        // Act / Assert
        assert_eq!(bucket_ms(&query(3_600_000)), 40_000);
        assert_eq!(bucket_ms(&query(30_000)), 1_000);
    }
}
