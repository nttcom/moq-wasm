use std::{future, path::PathBuf, sync::Arc};

use anyhow::Context;
use app_registry::{Location, LocationVariables, RegistryStore};
use console::{ConsoleServer, ConsoleState, GOOGLE_IAP_KEYS_URL, Identity, IdentitySettings};
use reqwest::Url;
use tokio::net::TcpListener;
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .init();

    let port: u16 = non_empty_env("PORT")
        .unwrap_or_else(|| "8080".to_owned())
        .parse()
        .context("PORT must be a port number")?;
    let identity = Identity::new(
        IdentitySettings {
            iap_audience: non_empty_env("CONSOLE_IAP_AUDIENCE"),
            development_sub: non_empty_env("CONSOLE_DEV_SUB"),
            on_cloud_run: non_empty_env("K_SERVICE").is_some(),
        },
        Url::parse(GOOGLE_IAP_KEYS_URL).expect("GOOGLE_IAP_KEYS_URL is a valid URL"),
    )?;
    let location = Location::from_env(&LocationVariables {
        file: "CONSOLE_REGISTRY_FILE",
        bucket: "CONSOLE_REGISTRY_BUCKET",
        object: "CONSOLE_REGISTRY_OBJECT",
    })?
    .context("set CONSOLE_REGISTRY_FILE or CONSOLE_REGISTRY_BUCKET")?;
    let store = RegistryStore::new(location)?;
    let apps = store.load().await?.apps;
    tracing::info!(apps = apps.len(), "registry loaded");

    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let static_dir = non_empty_env("CONSOLE_STATIC_DIR").map(PathBuf::from);
    let state = ConsoleState::new(identity, store, static_dir);
    let _server = ConsoleServer::run(listener, Arc::new(state));
    tracing::info!(port, "console listening");

    future::pending().await
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}
