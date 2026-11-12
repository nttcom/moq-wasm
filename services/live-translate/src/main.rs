mod broadcast_namespace;
mod broadcast_translation;
mod catalog_publication;
mod echo_translation;
mod gemini;
mod language_translation;
mod open_hours;
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
mod translation_policy;

use std::{future::pending, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use clap::Parser;
use metadata_token::MetadataToken;
use moqt::{CertificateVerification, ClientConfig};
use tokio::{net::TcpListener, sync::Semaphore};
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

use crate::{
    broadcast_namespace::BroadcastNamespaces,
    gemini::{AgentPlatform, GeminiCredentials},
    open_hours::OpenHours,
    root_translation::{RootTranslation, TranslationResources},
    session_server::{SessionServer, SessionServerState},
    target_language::find_by_code,
    temporary_apps::TemporaryApps,
    translation_engine::TranslationEngine,
    translation_policy::TranslationPolicy,
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
        env = "LIVE_TRANSLATE_OPEN_HOURS",
        help = "Translate only inside these weekly hours, like 'Mon-Fri 09:00-17:30 +09:00'; always when unset"
    )]
    open_hours: Option<OpenHours>,

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
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .init();
    let args = Args::parse();
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
        open_hours: args.open_hours.clone(),
        language_time_limit: Duration::from_secs(args.language_time_limit_minutes * 60),
        always_on,
    });
    let resources = TranslationResources {
        engine,
        capacity: Arc::new(Semaphore::new(args.max_sessions)),
        policy: policy.clone(),
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
                    policy: policy.clone(),
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
        return pending().await;
    }
    let config = ClientConfig {
        port: 0,
        certificate_verification: CertificateVerification::native_roots_or_disabled(!args.insecure),
        ..ClientConfig::default()
    };
    let namespaces = BroadcastNamespaces::new(BroadcastNamespaces::PUBLIC_ROOT.to_string());
    RootTranslation::connect(&args.relay, &config, namespaces, resources)
        .await?
        .route_events(None)
        .await
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
        None => GeminiCredentials::MetadataServer(MetadataToken::new(reqwest::Client::new())),
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
