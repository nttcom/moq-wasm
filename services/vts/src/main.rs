use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use app_registry::{Location, LocationVariables, RegistryStore};
use telemetry_otlp::non_empty_env;
use tokio::net::TcpListener;
use vts::{Registry, Secrets, VtsServer};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = telemetry_otlp::init(telemetry_otlp::TelemetryConfig {
        service_name: "vts",
        service_version: env!("CARGO_PKG_VERSION"),
        default_directives: &["vts=info"],
        log_output: telemetry_otlp::LogOutput::Stdout,
        ..Default::default()
    })?;

    let port: u16 = non_empty_env("VTS_PORT")
        .unwrap_or_else(|| "8081".to_owned())
        .parse()
        .context("VTS_PORT must be a port number")?;

    let registry = Registry::load(RegistryStore::new(registry_location()?)?).await?;
    let temporary_app_secret = non_empty_env("VTS_TEMPORARY_APP_SECRET");

    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let _server = VtsServer::run(
        listener,
        Arc::new(Secrets {
            registry,
            temporary_app_secret,
        }),
    );
    tracing::info!(port, "vts listening");

    http_server::shutdown_signal().await?;
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
