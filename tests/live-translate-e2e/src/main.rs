mod source_streamer;
mod viewer_checks;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

use crate::source_streamer::SourceStreamer;

const RELAY_URL_ENV: &str = "MOQT_E2E_RELAY_URL";
const DEFAULT_RELAY_URL: &str = "moqt://127.0.0.1:4433";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .init();
    let relay_url = std::env::var(RELAY_URL_ENV).unwrap_or_else(|_| DEFAULT_RELAY_URL.to_string());
    let broadcast_namespace = format!("anon/live-translate/e2e-{}", unix_micros());
    let _streamer = SourceStreamer::run(&relay_url, &broadcast_namespace).await?;
    viewer_checks::run(&relay_url, &broadcast_namespace).await?;
    tracing::info!("live-translate E2E passed");
    Ok(())
}

pub(crate) fn unix_micros() -> u64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    u64::try_from(since_epoch.as_micros()).unwrap_or(u64::MAX)
}
