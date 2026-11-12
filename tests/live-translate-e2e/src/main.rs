mod source_streamer;
mod viewer_checks;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use moqt::{CertificateVerification, ClientConfig};
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

use crate::source_streamer::SourceStreamer;

const RELAY_URL_ENV: &str = "MOQT_E2E_RELAY_URL";
const DEFAULT_RELAY_URL: &str = "moqt://127.0.0.1:4433";
const ROOT_ENV: &str = "LIVE_TRANSLATE_E2E_ROOT";
const DEFAULT_ROOT: &str = "anon/live-translate";
/// The token of the app `ROOT_ENV` is under; the public root needs none.
const TOKEN_ENV: &str = "LIVE_TRANSLATE_E2E_TOKEN";

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
    let root = std::env::var(ROOT_ENV).unwrap_or_else(|_| DEFAULT_ROOT.to_string());
    let broadcast_namespace = format!("{root}/e2e-{}", unix_micros());
    let _streamer = SourceStreamer::run(&client_config(), &relay_url, &broadcast_namespace).await?;
    viewer_checks::run(&client_config(), &relay_url, &broadcast_namespace).await?;
    tracing::info!("live-translate E2E passed");
    Ok(())
}

fn client_config() -> ClientConfig {
    ClientConfig {
        certificate_verification: CertificateVerification::Disabled,
        authorization_token: std::env::var(TOKEN_ENV).ok().map(Into::into),
        ..ClientConfig::default()
    }
}

pub(crate) fn unix_micros() -> u64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    u64::try_from(since_epoch.as_micros()).unwrap_or(u64::MAX)
}
