mod bot_types;
mod bots;
mod launch_auth;
mod server;
mod signer;
mod store;
#[cfg(test)]
mod test_support;

use std::sync::Arc;

use anyhow::Context;
use app_registry::{Location, LocationVariables, RegistryStore};
use google_cloud::{GOOGLE_CERTS_URL, IdTokenVerifier, MetadataToken};
use telemetry_otlp::non_empty_env;
use tokio::net::TcpListener;

use crate::{
    bot_types::BotTypes, bots::Bots, launch_auth::LaunchAuth, server::BotDispatcherServer,
    signer::BotTokenSigner, store::BotStore,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = telemetry_otlp::init(telemetry_otlp::TelemetryConfig {
        service_name: "bot-dispatcher",
        service_version: env!("CARGO_PKG_VERSION"),
        default_directives: &["bot_dispatcher=info", "job_launcher=info"],
        log_output: telemetry_otlp::LogOutput::Stdout,
        ..Default::default()
    })?;

    let port: u16 = non_empty_env("PORT")
        .unwrap_or_else(|| "8098".to_owned())
        .parse()
        .context("PORT must be a port number")?;
    let required = |name: &str| non_empty_env(name).with_context(|| format!("{name} must be set"));
    let public_url = required("BOT_DISPATCHER_PUBLIC_URL")?;
    let types_text = required("BOT_DISPATCHER_TYPES")?;
    let metadata_token = Arc::new(MetadataToken::new(reqwest::Client::new()));
    let types = BotTypes::parse(&types_text, &metadata_token)?;
    let store = BotStore::open(&required("BOT_DISPATCHER_STORE_URL")?.parse()?)?;
    // Without authorization the bots join with no token, so no app secret is read.
    let insecure =
        non_empty_env("BOT_DISPATCHER_INSECURE_SKIP_AUTH").is_some_and(|value| value == "true");
    let (signer, launch_auth) = if insecure {
        tracing::warn!(
            "BOT_DISPATCHER_INSECURE_SKIP_AUTH: every request is accepted and bots join with no token"
        );
        (None, LaunchAuth::Insecure)
    } else {
        let registry = Location::from_env(&LocationVariables {
            file: "BOT_DISPATCHER_REGISTRY_FILE",
            bucket: "BOT_DISPATCHER_REGISTRY_BUCKET",
            object: "BOT_DISPATCHER_REGISTRY_OBJECT",
        })?
        .context("set BOT_DISPATCHER_REGISTRY_FILE or BOT_DISPATCHER_REGISTRY_BUCKET")?;
        let registry = RegistryStore::new(registry)?;
        // Fail at startup rather than on the first launch request.
        let apps = registry.load().await?;
        tracing::info!(apps = apps.len(), "registry loaded");
        let signer = BotTokenSigner::new(
            registry,
            non_empty_env("BOT_DISPATCHER_TEMPORARY_APP_SECRET"),
        );
        (
            Some(signer),
            LaunchAuth::new(required("BOT_DISPATCHER_VTS_URL")?),
        )
    };
    let id_tokens = non_empty_env("BOT_DISPATCHER_JOB_SERVICE_ACCOUNT")
        .map(|service_accounts| -> anyhow::Result<IdTokenVerifier> {
            let certs_url = non_empty_env("BOT_DISPATCHER_GOOGLE_CERTS_URL")
                .unwrap_or_else(|| GOOGLE_CERTS_URL.to_owned())
                .parse()
                .context("BOT_DISPATCHER_GOOGLE_CERTS_URL must be a URL")?;
            IdTokenVerifier::new(
                certs_url,
                public_url.clone(),
                service_accounts
                    .split(',')
                    .map(str::trim)
                    .filter(|account| !account.is_empty())
                    .map(str::to_owned)
                    .collect(),
            )
        })
        .transpose()?;
    let bots = Bots::new(types, store, signer, id_tokens, public_url);

    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let _server = BotDispatcherServer::run(listener, bots, launch_auth);
    tracing::info!(port, "bot-dispatcher listening");

    http_server::shutdown_signal().await?;
    tracing::info!("bot-dispatcher shutting down");
    Ok(())
}
