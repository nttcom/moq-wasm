use relay_stats::RelaySnapshot;
use serde::Deserialize;

use crate::{clickhouse::ClickHouse, schema::SNAPSHOTS};

const RELAY_SILENCE_MS: u64 = 10_000;

#[derive(Deserialize)]
struct PayloadRow {
    payload: String,
}

pub async fn snapshots_at(
    clickhouse: &ClickHouse,
    at_ms: u64,
) -> anyhow::Result<Vec<RelaySnapshot>> {
    let rows: Vec<PayloadRow> = clickhouse
        .select(
            &format!(
                "SELECT argMax(payload, timestamp_ms) AS payload FROM {database}.{SNAPSHOTS} \
                 WHERE timestamp_ms <= {{at:UInt64}} AND timestamp_ms >= {{since:UInt64}} \
                 GROUP BY relay_id ORDER BY relay_id",
                database = clickhouse.database(),
            ),
            &[
                ("at", at_ms.to_string()),
                ("since", at_ms.saturating_sub(RELAY_SILENCE_MS).to_string()),
            ],
        )
        .await?;
    rows.iter()
        .map(|row| RelaySnapshot::from_json(row.payload.as_bytes()))
        .collect()
}
