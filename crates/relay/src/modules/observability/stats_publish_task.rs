use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use bytes::Bytes;
use moqt::{PublishOption, TrackWriter};
use tokio::{task::JoinHandle, time::MissedTickBehavior};

use crate::modules::observability::{process_memory, stats_collector::StatsCollector};

const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(1);
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

pub(crate) struct StatsPublishConfig {
    pub(crate) inner_port: u16,
    pub(crate) relay_token: String,
}

pub struct StatsPublishTask {
    join_handle: JoinHandle<()>,
}

fn unix_millis_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

impl StatsPublishTask {
    pub(crate) fn run(collector: StatsCollector, config: StatsPublishConfig) -> Self {
        let join_handle = tokio::spawn(async move {
            loop {
                if let Err(error) = publish_snapshots(&collector, &config).await {
                    tracing::warn!(
                        ?error,
                        relay_id = collector.relay_id(),
                        "stats publishing stopped; reconnecting"
                    );
                }
                tokio::time::sleep(RECONNECT_DELAY).await;
            }
        });
        Self { join_handle }
    }
}

impl Drop for StatsPublishTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

async fn publish_snapshots(
    collector: &StatsCollector,
    config: &StatsPublishConfig,
) -> anyhow::Result<()> {
    let endpoint = moqt::Endpoint::<moqt::QUIC>::create_client(&moqt::ClientConfig {
        port: 0,
        verify_certificate: false,
        authorization_token: Some(config.relay_token.clone()),
    })?;
    let session = endpoint
        .connect(&format!("moqt://127.0.0.1:{}", config.inner_port))
        .await?
        .await
        .context("loopback session to the inner endpoint failed")?;
    let publisher = session.publisher();
    let namespace = relay_stats::track_namespace(collector.relay_id());
    let subscription = publisher
        .publish(
            namespace.clone(),
            relay_stats::TRACK_NAME.to_string(),
            PublishOption::default(),
        )
        .await
        .context("PUBLISH of the stats track was refused")?;
    tracing::info!(%namespace, track_name = relay_stats::TRACK_NAME, "publishing relay stats");
    let mut writer = TrackWriter::new(publisher.create_stream(&subscription), unix_millis_now());
    let mut ticker = tokio::time::interval(SNAPSHOT_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        let timestamp_ms = unix_millis_now();
        let snapshot = collector
            .collect(timestamp_ms, process_memory::resident_set_bytes().await)
            .await;
        writer
            .start_group_at(timestamp_ms.max(writer.next_group_id()))
            .await?;
        writer
            .write(Bytes::from(snapshot.to_json()), vec![])
            .await?;
    }
}
