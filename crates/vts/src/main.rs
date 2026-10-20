use std::{future, path::PathBuf, sync::Arc};

use anyhow::{Context, bail};
use app_registry::{Endpoint, Location, RegistryStore};
use tokio::net::TcpListener;
use tracing_subscriber::{EnvFilter, filter::LevelFilter};
use vts::{Registry, VtsServer};

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

    let registry = Registry::load(RegistryStore::new(registry_location()?)?).await?;

    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let _server = VtsServer::run(listener, Arc::new(registry));
    tracing::info!(port, "vts listening");

    future::pending().await
}

fn registry_location() -> anyhow::Result<Location> {
    match (
        non_empty_env("VTS_REGISTRY_BUCKET"),
        non_empty_env("VTS_APPS_FILE"),
    ) {
        (Some(_), Some(_)) => bail!("set VTS_REGISTRY_BUCKET or VTS_APPS_FILE, not both"),
        (Some(bucket), None) => Ok(Location::CloudStorage {
            endpoint: Endpoint::from_env()?,
            bucket,
            object: env_or("VTS_REGISTRY_OBJECT", "registry.json"),
        }),
        (None, apps_file) => Ok(Location::File(PathBuf::from(
            apps_file.unwrap_or_else(|| "/etc/vts/apps.json".to_owned()),
        ))),
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn env_or(name: &str, default: &str) -> String {
    non_empty_env(name).unwrap_or_else(|| default.to_owned())
}
