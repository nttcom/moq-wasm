use std::{future, path::PathBuf, sync::Arc};

use anyhow::Context;
use app_registry::{Location, LocationVariables, RegistryStore};
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

    let port: u16 = std::env::var("VTS_PORT")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "8081".to_owned())
        .parse()
        .context("VTS_PORT must be a port number")?;

    let registry = Registry::load(RegistryStore::new(registry_location()?)?).await?;

    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let _server = VtsServer::run(listener, Arc::new(registry));
    tracing::info!(port, "vts listening");

    future::pending().await
}

fn registry_location() -> anyhow::Result<Location> {
    let variables = LocationVariables {
        file: "VTS_APPS_FILE",
        bucket: "VTS_REGISTRY_BUCKET",
        object: "VTS_REGISTRY_OBJECT",
    };
    Ok(Location::from_env(&variables)?
        .unwrap_or_else(|| Location::File(PathBuf::from("/etc/vts/apps.json"))))
}
