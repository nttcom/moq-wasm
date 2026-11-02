use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use telemetry::relay_stats::RelaySnapshot;

use crate::{clickhouse::ClickHouse, schema::SNAPSHOTS};

/// How long a relay may go without a snapshot before it leaves the view.
pub const RELAY_SILENCE: Duration = Duration::from_secs(10);

/// The snapshots the browser shows at a past time, and the earlier ones it
/// derives rates from.
pub struct PastSnapshots {
    pub snapshots: Vec<RelaySnapshot>,
    pub previous: Vec<RelaySnapshot>,
}

#[derive(Deserialize)]
struct PayloadRow {
    current_payload: Option<String>,
    previous_payload: Option<String>,
}

/// Each relay's last snapshot at or before `at_ms`, and its last at or before
/// `previous_ms`, each only if published within `RELAY_SILENCE` before that time.
/// The caller keeps `previous_ms` within `RELAY_SILENCE` of `at_ms`: one scan
/// reads both, and this keeps it short.
pub async fn snapshots_at(
    clickhouse: &ClickHouse,
    at_ms: u64,
    previous_ms: u64,
) -> anyhow::Result<PastSnapshots> {
    let silence_ms = RELAY_SILENCE.as_millis() as u64;
    let rows: Vec<PayloadRow> = clickhouse
        .select(
            &format!(
                "SELECT \
                   argMaxIfOrNull(payload, timestamp_ms, timestamp_ms >= {{since:UInt64}}) \
                     AS current_payload, \
                   argMaxIfOrNull(payload, timestamp_ms, timestamp_ms <= {{previous:UInt64}}) \
                     AS previous_payload \
                 FROM {database}.{SNAPSHOTS} \
                 WHERE timestamp_ms <= {{at:UInt64}} AND timestamp_ms >= {{previous_since:UInt64}} \
                 GROUP BY relay_id ORDER BY relay_id",
                database = clickhouse.database(),
            ),
            &[
                ("at", at_ms.to_string()),
                ("since", at_ms.saturating_sub(silence_ms).to_string()),
                ("previous", previous_ms.to_string()),
                (
                    "previous_since",
                    previous_ms.saturating_sub(silence_ms).to_string(),
                ),
            ],
        )
        .await?;
    let decoded = |payload: &Option<String>| {
        payload
            .as_ref()
            .map(|payload| RelaySnapshot::from_json(payload.as_bytes()))
    };
    Ok(PastSnapshots {
        snapshots: rows
            .iter()
            .filter_map(|row| decoded(&row.current_payload))
            .collect::<Result<_, _>>()?,
        previous: rows
            .iter()
            .filter_map(|row| decoded(&row.previous_payload))
            .collect::<Result<_, _>>()?,
    })
}

/// Each relay's newest snapshot, if it published one within `RELAY_SILENCE`,
/// so a stopped relay leaves the live view instead of freezing on its last
/// state.
pub async fn latest_snapshots(clickhouse: &ClickHouse) -> anyhow::Result<Vec<RelaySnapshot>> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?;
    Ok(snapshots_at(clickhouse, now_ms, now_ms).await?.snapshots)
}
