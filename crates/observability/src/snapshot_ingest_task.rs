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
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::mpsc,
        time,
    };

    use super::{FLUSH_INTERVAL, SnapshotIngestTask};
    use crate::{
        clickhouse::ClickHouse, config::ClickHouseConfig, latest_snapshots::LatestSnapshots,
        test_support::snapshot,
    };

    async fn fake_clickhouse() -> (ClickHouse, mpsc::UnboundedReceiver<(String, String)>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (insert_sender, insert_receiver) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let insert_sender = insert_sender.clone();
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let (head_end, content_length) = loop {
                        let mut chunk = [0; 4096];
                        let read = stream.read(&mut chunk).await.unwrap();
                        request.extend_from_slice(&chunk[..read]);
                        let text = String::from_utf8_lossy(&request).to_string();
                        if let Some(head_end) = text.find("\r\n\r\n") {
                            let length = text[..head_end]
                                .to_ascii_lowercase()
                                .lines()
                                .find_map(|line| {
                                    line.strip_prefix("content-length: ").map(str::to_string)
                                })
                                .map_or(0, |length| length.parse::<usize>().unwrap());
                            break (head_end + 4, length);
                        }
                    };
                    while request.len() < head_end + content_length {
                        let mut chunk = [0; 4096];
                        let read = stream.read(&mut chunk).await.unwrap();
                        request.extend_from_slice(&chunk[..read]);
                    }
                    let text = String::from_utf8_lossy(&request).to_string();
                    let request_line = text.lines().next().unwrap().to_string();
                    insert_sender
                        .send((request_line, text[head_end..].to_string()))
                        .unwrap();
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                        )
                        .await
                        .unwrap();
                });
            }
        });
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
        let (request_line, body) = insert_receiver.recv().await.unwrap();

        // Assert: both snapshots arrive in one INSERT
        assert!(request_line.contains("INSERT"), "{request_line}");
        assert_eq!(body.lines().count(), 2, "{body}");
    }
}
