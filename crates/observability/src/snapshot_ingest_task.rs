use std::{mem, time::Duration};

use relay_stats::RelaySnapshot;
use tokio::{
    sync::mpsc,
    task::{JoinHandle, JoinSet},
    time,
};

use crate::{
    clickhouse::ClickHouse, latest_snapshots::LatestSnapshots, snapshot_rows::SnapshotRows,
};

const MAX_STORES_IN_FLIGHT: usize = 16;

const FLUSH_INTERVAL: Duration = Duration::from_secs(30);

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
            let mut buffered = SnapshotRows::default();
            let mut flush_ticker = time::interval(FLUSH_INTERVAL);
            loop {
                tokio::select! {
                    received = snapshot_receiver.recv() => {
                        let Some(snapshot) = received else { break };
                        latest.update(snapshot.clone());
                        if let Err(error) = buffered.push(&snapshot) {
                            tracing::warn!(?error, relay_id = %snapshot.relay_id, "snapshot was not buffered");
                        }
                    }
                    _ = flush_ticker.tick() => {
                        let rows = mem::take(&mut buffered);
                        if stores.len() >= MAX_STORES_IN_FLIGHT {
                            stores.join_next().await;
                        }
                        let clickhouse = clickhouse.clone();
                        stores.spawn(async move {
                            if let Err(error) = rows.store(&clickhouse).await {
                                tracing::warn!(?error, "snapshots were not stored");
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

#[cfg(test)]
mod tests {
    use tokio::{sync::mpsc, time};

    use super::{FLUSH_INTERVAL, SnapshotIngestTask};
    use crate::{
        clickhouse::ClickHouse,
        config::ClickHouseConfig,
        latest_snapshots::LatestSnapshots,
        test_support::{fake_http_server, snapshot},
    };

    async fn fake_clickhouse() -> (ClickHouse, mpsc::UnboundedReceiver<(String, String)>) {
        let (url, insert_receiver) = fake_http_server().await;
        let clickhouse = ClickHouse::new(ClickHouseConfig {
            url,
            database: "obs".to_string(),
            user: None,
            password: None,
        });
        (clickhouse, insert_receiver)
    }

    #[tokio::test(start_paused = true)]
    async fn snapshots_of_one_interval_are_inserted_together_when_it_elapses() {
        // Arrange
        let (clickhouse, mut insert_receiver) = fake_clickhouse().await;
        let (snapshot_sender, snapshot_receiver) = mpsc::channel(8);
        let _ingest =
            SnapshotIngestTask::run(snapshot_receiver, LatestSnapshots::default(), clickhouse);

        // Act
        snapshot_sender.send(snapshot(1)).await.unwrap();
        snapshot_sender.send(snapshot(2)).await.unwrap();
        time::advance(FLUSH_INTERVAL / 2).await;

        // Assert: nothing is inserted before the interval elapses
        assert!(insert_receiver.try_recv().is_err());

        // Act
        time::advance(FLUSH_INTERVAL).await;
        let (uri, body) = insert_receiver.recv().await.unwrap();

        // Assert: both snapshots arrive in one INSERT
        assert!(uri.contains("INSERT"), "{uri}");
        assert_eq!(body.lines().count(), 2, "{body}");
    }
}
