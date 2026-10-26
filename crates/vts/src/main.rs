use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use app_registry::{Location, LocationVariables, RegistryStore};
use tokio::{
    net::TcpListener,
    signal::unix::{SignalKind, signal},
};
use vts::{Registry, VtsServer};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = telemetry::init(telemetry::TelemetryConfig {
        service_name: "vts",
        service_version: env!("CARGO_PKG_VERSION"),
        default_directives: &["vts=info"],
        resource_attributes: Vec::new(),
    })?;

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

    // As PID 1 in a container the process has no default SIGTERM action, so
    // `docker stop` and Cloud Run would otherwise wait for their SIGKILL.
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result?,
        _ = terminate.recv() => {}
    }
    tracing::info!("vts shutting down");
    Ok(())
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
