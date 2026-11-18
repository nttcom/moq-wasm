use std::time::Duration;

use anyhow::Context;
use telemetry_otlp::non_empty_env;
use temporary_apps::{RateLimit, TemporaryApps, TemporaryAppsServer};
use tokio::{
    net::TcpListener,
    signal::unix::{SignalKind, signal},
};

/// New temporary apps one client address may get per window.
const MAX_ISSUES_PER_WINDOW: u32 = 30;
const ISSUE_WINDOW: Duration = Duration::from_secs(60 * 60);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = telemetry_otlp::init(telemetry_otlp::TelemetryConfig {
        service_name: "temporary-apps",
        service_version: env!("CARGO_PKG_VERSION"),
        default_directives: &["temporary_apps=info"],
        log_output: telemetry_otlp::LogOutput::Stdout,
        ..Default::default()
    })?;

    let port: u16 = non_empty_env("PORT")
        .unwrap_or_else(|| "8083".to_owned())
        .parse()
        .context("PORT must be a port number")?;
    let secret =
        non_empty_env("TEMPORARY_APPS_SECRET").context("TEMPORARY_APPS_SECRET must be set")?;
    let proxy_hops = non_empty_env("TEMPORARY_APPS_TRUSTED_PROXY_HOPS")
        .map(|hops| hops.parse())
        .transpose()
        .context("TEMPORARY_APPS_TRUSTED_PROXY_HOPS must be a number")?
        .unwrap_or(0);

    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let _server = TemporaryAppsServer::run(
        listener,
        TemporaryApps::new(secret),
        RateLimit::new(MAX_ISSUES_PER_WINDOW, ISSUE_WINDOW),
        proxy_hops,
    );
    tracing::info!(port, "temporary-apps listening");

    // As PID 1 in a container the process has no default SIGTERM action, so
    // `docker stop` and Cloud Run would otherwise wait for their SIGKILL.
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result?,
        _ = terminate.recv() => {}
    }
    tracing::info!("temporary-apps shutting down");
    Ok(())
}
