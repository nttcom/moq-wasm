mod broadcast_namespace;
mod broadcast_translation;
mod catalog_track;
mod echo_translation;
mod gemini;
mod language_translation;
mod opus_codec;
mod pcm_framer;
mod root_translation;
mod session_server;
mod source_audio;
mod source_catalog;
mod target_language;
mod temporary_apps;
mod translated_audio;
mod translated_catalog;
mod translation_engine;

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use clap::Parser;
use moq_wasm::{
    CertificateVerification, ConnectOptions,
    telemetry::{self, Telemetry, TelemetryOptions},
};
use tokio::{net::TcpListener, sync::Semaphore};
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    broadcast_namespace::BroadcastNamespaces,
    gemini::{AgentPlatform, GeminiCredentials, MetadataToken},
    root_translation::{TranslationPolicy, TranslationResources, translate_root},
    session_server::{SessionServer, SessionServerState},
    target_language::find_by_code,
    temporary_apps::TemporaryApps,
    translation_engine::TranslationEngine,
};

#[derive(Parser, Debug)]
#[command(about = "Translates live broadcasts on a MoQT relay with Gemini Live Translate")]
struct Args {
    #[arg(
        long,
        env = "LIVE_TRANSLATE_RELAY_URL",
        default_value = "moqt://localhost:4433"
    )]
    relay: String,

    #[arg(long)]
    insecure: bool,

    #[arg(
        long,
        env = "PORT",
        help = "Serve the requests of the streamers' pages on this port: a temporary app to translate, and the stream that keeps the instance running"
    )]
    http_port: Option<u16>,

    #[arg(
        long,
        help = "Also translate the broadcasts under the public anon/live-translate, which need no token: for local development and the e2e suites"
    )]
    public_root: bool,

    #[arg(
        long,
        env = "LIVE_TRANSLATE_GCP_PROJECT",
        help = "The Google Cloud project whose Agent Platform serves Gemini"
    )]
    gcp_project: Option<String>,

    #[arg(
        long,
        env = "LIVE_TRANSLATE_GCP_LOCATION",
        default_value = "global",
        help = "The Agent Platform location; the translate model is served from global only"
    )]
    gcp_location: String,

    #[arg(
        long,
        env = "LIVE_TRANSLATE_GCLOUD_IMPERSONATE",
        value_name = "SERVICE_ACCOUNT_EMAIL",
        help = "Impersonate this service account through gcloud for a fresh token per connection, for running outside Google Cloud; on Cloud Run the metadata server is used instead"
    )]
    gcloud_impersonate: Option<String>,

    #[arg(
        long,
        help = "Return the source audio after each language's fixed delay instead of calling Gemini, to test without Google Cloud"
    )]
    echo: bool,

    #[arg(long, env = "LIVE_TRANSLATE_MAX_SESSIONS", default_value_t = 5)]
    max_sessions: usize,

    #[arg(
        long,
        env = "LIVE_TRANSLATE_MAX_APPS",
        default_value_t = 5,
        help = "Temporary apps translated at once, each over a relay session of its own"
    )]
    max_apps: usize,

    #[arg(
        long,
        env = "LIVE_TRANSLATE_LANGUAGE_TIME_LIMIT_MINUTES",
        default_value_t = 60,
        help = "Stop a language's translation after this many minutes at a stretch; its viewers may subscribe again"
    )]
    language_time_limit_minutes: u64,

    #[arg(
        long,
        env = "LIVE_TRANSLATE_ALWAYS_ON",
        value_delimiter = ',',
        value_name = "LANGUAGE_CODES",
        help = "Languages translated for the whole broadcast whether or not anyone listens, like 'en,es', so their first viewer hears them at once; each costs a Gemini session"
    )]
    always_on: Vec<String>,

    #[arg(
        long,
        env = "LIVE_TRANSLATE_TELEMETRY_URL",
        value_name = "OBSERVABILITY_URL",
        help = "Also send the logs and spans to this observability server's client telemetry, filed under each temporary app's token"
    )]
    telemetry_url: Option<String>,
}

const LOG_DIRECTIVES: &str = "warn,live_translate=info";

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let telemetry = init_logging(args.telemetry_url.as_deref());
    let result = run(args).await;
    if let Some(telemetry) = telemetry {
        telemetry.flush().await;
    }
    result
}

async fn run(args: Args) -> Result<()> {
    let engine = translation_engine(&args)?;
    let always_on = args
        .always_on
        .iter()
        .filter(|code| !code.is_empty())
        .map(|code| {
            find_by_code(code).with_context(|| format!("--always-on: unknown language {code}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let policy = Arc::new(TranslationPolicy {
        language_time_limit: Duration::from_secs(args.language_time_limit_minutes * 60),
        always_on,
    });
    let resources = TranslationResources {
        engine,
        capacity: Arc::new(Semaphore::new(args.max_sessions)),
        policy,
    };
    let _session_server = match args.http_port {
        Some(port) => {
            let listener = TcpListener::bind(("0.0.0.0", port))
                .await
                .with_context(|| format!("listen on port {port}"))?;
            tracing::info!(port, "serving the pages' requests");
            Some(SessionServer::run(
                listener,
                SessionServerState {
                    temporary_apps: TemporaryApps::new(
                        args.relay.clone(),
                        args.insecure,
                        resources.clone(),
                        args.max_apps,
                    ),
                },
            ))
        }
        None => None,
    };
    if !args.public_root {
        if args.http_port.is_none() {
            bail!("nothing to translate: set PORT to serve temporary apps, or pass --public-root");
        }
        tracing::info!("waiting for temporary apps");
        shutdown_signal().await?;
        tracing::info!("live-translate shutting down");
        return Ok(());
    }
    let options = ConnectOptions {
        certificate_verification: CertificateVerification::native_roots_or_disabled(!args.insecure),
        ..ConnectOptions::default()
    };
    let session = Arc::new(moq_wasm::connect(&args.relay, options).await?);
    tracing::info!(url = args.relay, "connected to relay");
    let namespaces = BroadcastNamespaces::new(BroadcastNamespaces::PUBLIC_ROOT.to_string());
    tokio::select! {
        result = translate_root(session.clone(), namespaces, resources, None) => {
            result?;
            bail!("relay closed the session")
        }
        signal = shutdown_signal() => {
            signal?;
            tracing::info!("live-translate shutting down");
            session.close().await;
            Ok(())
        }
    }
}

fn init_logging(telemetry_url: Option<&str>) -> Option<Telemetry> {
    let filter = || EnvFilter::try_from_default_env().unwrap_or_else(|_| LOG_DIRECTIVES.into());
    let (telemetry_layer, telemetry) = match telemetry_url {
        Some(url) => {
            let (layer, telemetry) = telemetry::layer(TelemetryOptions {
                url: url.to_string(),
                page: "live-translate".to_string(),
            });
            (Some(layer.with_filter(filter())), Some(telemetry))
        }
        None => (None, None),
    };
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(filter()))
        .with(telemetry_layer)
        .init();
    telemetry
}

/// Waits for `SIGINT` (Ctrl-C) or `SIGTERM`. As PID 1 in a container the
/// process has no default `SIGTERM` action, so Cloud Run would otherwise wait
/// for `SIGKILL` and drop buffered telemetry.
async fn shutdown_signal() -> std::io::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result,
        _ = terminate.recv() => Ok(()),
    }
}

fn translation_engine(args: &Args) -> Result<TranslationEngine> {
    if args.echo {
        tracing::warn!("--echo: returning the source audio instead of translating");
        return Ok(TranslationEngine::Echo);
    }
    let Some(project) = args
        .gcp_project
        .as_deref()
        .filter(|project| !project.is_empty())
    else {
        bail!("LIVE_TRANSLATE_GCP_PROJECT is not set; pass --echo to run without Gemini");
    };
    let credentials = match args
        .gcloud_impersonate
        .as_deref()
        .filter(|account| !account.is_empty())
    {
        Some(service_account) => GeminiCredentials::Gcloud {
            service_account: service_account.to_string(),
        },
        None => GeminiCredentials::MetadataServer(MetadataToken::new()),
    };
    let platform = AgentPlatform::new(project, &args.gcp_location, credentials);
    tracing::info!(
        endpoint = %platform.endpoint,
        model = %platform.model,
        credentials = platform.credentials_source(),
        "Gemini on the Agent Platform"
    );
    Ok(TranslationEngine::Gemini {
        platform: Arc::new(platform),
    })
}
