use std::time::Duration;

use relay_stats::RelaySnapshot;
use tokio::{
    sync::mpsc,
    task::{JoinHandle, JoinSet},
    time::MissedTickBehavior,
};

use crate::{clickhouse::ClickHouse, latest_snapshots::LatestSnapshots, snapshot_rows};

// ClickHouse turns every insert into a part and merges it into its partition, so the
// relays' once-per-second snapshots are written as one insert per table per flush.
const FLUSH_INTERVAL: Duration = Duration::from_secs(10);
const MAX_STORES_IN_FLIGHT: usize = 4;

pub struct SnapshotIngestTask {
    join_handle: JoinHandle<()>,
}

impl SnapshotIngestTask {
    pub fn run(
        mut snapshot_receiver: mpsc::Receiver<RelaySnapshot>,
        latest: LatestSnapshots,
        clickhouse: ClickHouse,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let mut stores = JoinSet::new();
            let mut batch = Vec::new();
            let mut flush = tokio::time::interval(FLUSH_INTERVAL);
            flush.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    snapshot = snapshot_receiver.recv() => {
                        let Some(snapshot) = snapshot else { break };
                        latest.update(snapshot.clone());
                        batch.push(snapshot);
                    }
                    _ = flush.tick() => {
                        if batch.is_empty() {
                            continue;
                        }
                        if stores.len() >= MAX_STORES_IN_FLIGHT {
                            stores.join_next().await;
                        }
                        let snapshots = std::mem::take(&mut batch);
                        let clickhouse = clickhouse.clone();
                        stores.spawn(async move {
                            if let Err(error) = snapshot_rows::store(&clickhouse, &snapshots).await {
                                tracing::warn!(?error, snapshots = snapshots.len(), "snapshots were not stored");
                            }
                        });
                        while stores.try_join_next().is_some() {}
                    }
                }
            }
        });
        Self { join_handle }
    }
}

impl Drop for SnapshotIngestTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
