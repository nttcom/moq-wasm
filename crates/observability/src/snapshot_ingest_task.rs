use relay_stats::RelaySnapshot;
use tokio::{
    sync::mpsc,
    task::{JoinHandle, JoinSet},
};

use crate::{clickhouse::ClickHouse, latest_snapshots::LatestSnapshots, snapshot_rows};

const MAX_STORES_IN_FLIGHT: usize = 16;

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
            while let Some(snapshot) = snapshot_receiver.recv().await {
                latest.update(snapshot.clone());
                if stores.len() >= MAX_STORES_IN_FLIGHT {
                    stores.join_next().await;
                }
                let clickhouse = clickhouse.clone();
                stores.spawn(async move {
                    if let Err(error) = snapshot_rows::store(&clickhouse, &snapshot).await {
                        tracing::warn!(?error, relay_id = %snapshot.relay_id, "snapshot was not stored");
                    }
                });
                while stores.try_join_next().is_some() {}
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
