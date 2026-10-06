use relay_stats::RelaySnapshot;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{clickhouse::ClickHouse, latest_snapshots::LatestSnapshots, snapshot_rows};

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
            while let Some(snapshot) = snapshot_receiver.recv().await {
                if let Err(error) = snapshot_rows::store(&clickhouse, &snapshot).await {
                    tracing::warn!(?error, relay_id = %snapshot.relay_id, "snapshot was not stored");
                }
                latest.update(snapshot);
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
