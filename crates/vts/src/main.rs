use std::{future, path::PathBuf, sync::Arc};

use anyhow::Context;
use tokio::net::TcpListener;
use tracing_subscriber::{EnvFilter, filter::LevelFilter};
use vts::{VtsServer, load_apps};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .init();

    let port: u16 = env_or("VTS_PORT", "8081")
        .parse()
        .context("VTS_PORT must be a port number")?;
    let apps_file = PathBuf::from(env_or("VTS_APPS_FILE", "/etc/vts/apps.json"));

    let apps = Arc::new(load_apps(&apps_file).await?);
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let _server = VtsServer::run(listener, apps.clone());
    tracing::info!(port, apps = apps.len(), "vts listening");

    future::pending().await
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}
