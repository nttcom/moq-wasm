use std::future;

use observability::{
    clickhouse::ClickHouse,
    config::ObservabilityConfig,
    latest_snapshots::LatestSnapshots,
    relay_subscription_task::{RelayConnectionOptions, RelaySubscriptionTask},
    schema,
    snapshot_ingest_task::SnapshotIngestTask,
};
use tokio::sync::mpsc;
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

const SNAPSHOT_QUEUE: usize = 64;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .init();
    let config = ObservabilityConfig::from_env()?;
    let clickhouse = ClickHouse::new(config.clickhouse.clone());
    for statement in schema::statements(clickhouse.database()) {
        clickhouse.execute(&statement).await?;
    }

    let latest = LatestSnapshots::default();
    let (snapshot_sender, snapshot_receiver) = mpsc::channel(SNAPSHOT_QUEUE);
    let _ingest = SnapshotIngestTask::run(snapshot_receiver, latest.clone(), clickhouse.clone());
    let _subscriptions: Vec<RelaySubscriptionTask> = config
        .relays
        .iter()
        .map(|relay| {
            RelaySubscriptionTask::run(
                relay.clone(),
                RelayConnectionOptions {
                    auth_token: config.auth_token.clone(),
                    verify_certificate: config.verify_relay_certificate,
                },
                snapshot_sender.clone(),
            )
        })
        .collect();
    tracing::info!(relays = config.relays.len(), "observability started");

    future::pending().await
}
