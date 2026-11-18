mod bot_types;
mod bots;
mod id_token;
mod launch_auth;
mod server;
mod signer;
mod store;
#[cfg(test)]
mod test_support;

use std::sync::Arc;

use anyhow::Context;
use app_registry::{Location, LocationVariables, RegistryStore};
use google_cloud::MetadataToken;
use telemetry_otlp::non_empty_env;
use tokio::{
    net::TcpListener,
    signal::unix::{SignalKind, signal},
};

use crate::{
    bot_types::BotTypes,
    bots::Bots,
    id_token::{GOOGLE_CERTS_URL, IdTokenVerifier},
    launch_auth::LaunchAuth,
    server::BotsServer,
    signer::BotTokenSigner,
    store::BotStore,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = telemetry_otlp::init(telemetry_otlp::TelemetryConfig {
        service_name: "bots",
        service_version: env!("CARGO_PKG_VERSION"),
        default_directives: &["bots=info", "job_launcher=info"],
        log_output: telemetry_otlp::LogOutput::Stdout,
        ..Default::default()
    })?;

    let port: u16 = non_empty_env("PORT")
        .unwrap_or_else(|| "8098".to_owned())
        .parse()
        .context("PORT must be a port number")?;
    let required = |name: &str| non_empty_env(name).with_context(|| format!("{name} must be set"));
    let public_url = required("BOTS_PUBLIC_URL")?;
    let types_text = required("BOTS_TYPES")?;
    let metadata_token = Arc::new(MetadataToken::new(reqwest::Client::new()));
    let types = BotTypes::parse(&types_text, &metadata_token)?;
    let store = BotStore::open(&required("BOTS_STORE_URL")?.parse()?)?;
    // Without authorization the bots join with no token, so no app secret is read.
    let insecure = non_empty_env("BOTS_INSECURE_SKIP_AUTH").is_some_and(|value| value == "true");
    let (signer, launch_auth) = if insecure {
        tracing::warn!(
            "BOTS_INSECURE_SKIP_AUTH: every request is accepted and bots join with no token"
        );
        (None, LaunchAuth::Insecure)
    } else {
        let registry = Location::from_env(&LocationVariables {
            file: "BOTS_REGISTRY_FILE",
            bucket: "BOTS_REGISTRY_BUCKET",
            object: "BOTS_REGISTRY_OBJECT",
        })?
        .context("set BOTS_REGISTRY_FILE or BOTS_REGISTRY_BUCKET")?;
        let signer = BotTokenSigner::new(
            RegistryStore::new(registry)?,
            non_empty_env("BOTS_TEMPORARY_APP_SECRET"),
        );
        (Some(signer), LaunchAuth::new(required("BOTS_VTS_URL")?))
    };
    let id_tokens = non_empty_env("BOTS_JOB_SERVICE_ACCOUNT").map(|service_account| {
        IdTokenVerifier::new(
            non_empty_env("BOTS_GOOGLE_CERTS_URL").unwrap_or_else(|| GOOGLE_CERTS_URL.to_owned()),
            public_url.clone(),
            service_account,
        )
    });
    let bots = Bots::new(types, store, signer, id_tokens, public_url);

    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let _server = BotsServer::run(listener, bots, launch_auth);
    tracing::info!(port, "bots listening");

    // As PID 1 in a container the process has no default SIGTERM action.
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result?,
        _ = terminate.recv() => {}
    }
    tracing::info!("bots shutting down");
    Ok(())
}
