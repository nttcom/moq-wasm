mod app_access;
mod broadcast_namespace;
mod broadcast_translation;
mod catalog_publication;
mod echo_translation;
mod gemini;
mod language_translation;
mod open_hours;
mod opus_codec;
mod page_server;
mod pcm_framer;
mod source_audio;
mod source_catalog;
mod target_language;
mod translated_audio;
mod translated_catalog;
mod translation_engine;
mod translation_policy;

use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use clap::Parser;
use metadata_token::MetadataToken;
use moqt::{
    AuthorizationTokenSource, CertificateVerification, ClientConfig, Endpoint, QUIC,
    RequestErrorCode, Session, SessionEvent, namespace::NamespaceInterest,
    subscribe::SubscribeHandler,
};
use static_files::StaticFiles;
use tokio::{net::TcpListener, sync::Semaphore};
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

use crate::{
    app_access::AppAccess,
    broadcast_namespace::BroadcastNamespaces,
    broadcast_translation::BroadcastTranslation,
    gemini::{AgentPlatform, GeminiCredentials},
    open_hours::OpenHours,
    page_server::{PageServer, PageServerState},
    target_language::find_by_code,
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
        env = "LIVE_TRANSLATE_APP_ID",
        requires = "app_secret",
        help = "The relay app the broadcasts live under; without it they are read from the public anon/live-translate"
    )]
    app_id: Option<String>,

    #[arg(
        long,
        env = "LIVE_TRANSLATE_APP_SECRET",
        hide_env_values = true,
        requires = "app_id",
        help = "The app's secret, which signs the translator's and the pages' relay tokens"
    )]
    app_secret: Option<String>,

    #[arg(
        long,
        env = "PORT",
        help = "Serve the pages, their relay tokens and the keep-alive stream on this port; without it only the relay is served"
    )]
    http_port: Option<u16>,

    #[arg(
        long,
        env = "LIVE_TRANSLATE_STATIC_DIR",
        help = "The built pages to serve, with examples/live-translate/{streamer,viewer}/index.html and assets/ under it"
    )]
    static_dir: Option<PathBuf>,

    #[arg(
        long,
        env = "LIVE_TRANSLATE_PUBLIC_RELAY_URL",
        help = "The relay URL the pages connect to; defaults to --relay"
    )]
    public_relay_url: Option<String>,

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

struct Router {
    session: Arc<Session>,
    namespaces: BroadcastNamespaces,
    engine: TranslationEngine,
    capacity: Arc<Semaphore>,
    policy: Arc<TranslationPolicy>,
    broadcasts: HashMap<String, BroadcastTranslation>,
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
    let app = app_access(&args);
    let policy = Arc::new(TranslationPolicy {
        open_hours: args.open_hours.clone(),
        language_time_limit: Duration::from_secs(args.language_time_limit_minutes * 60),
        always_on,
    });
    let namespaces = BroadcastNamespaces::new(
        app.as_ref()
            .map(|app| app.app_id.clone())
            .unwrap_or_else(|| BroadcastNamespaces::PUBLIC_ROOT.to_string()),
    );
    let _page_server = match args.http_port {
        Some(port) => {
            let listener = TcpListener::bind(("0.0.0.0", port))
                .await
                .with_context(|| format!("listen on port {port}"))?;
            tracing::info!(port, "serving the pages");
            Some(PageServer::run(
                listener,
                PageServerState {
                    static_files: args.static_dir.clone().map(StaticFiles::new),
                    app: app.clone(),
                    namespaces: namespaces.clone(),
                    policy: policy.clone(),
                    public_relay_url: args
                        .public_relay_url
                        .clone()
                        .filter(|url| !url.is_empty())
                        .unwrap_or_else(|| args.relay.clone()),
                },
            ))
        }
        None => None,
    };
    let config = ClientConfig {
        port: 0,
        certificate_verification: CertificateVerification::native_roots_or_disabled(!args.insecure),
        authorization_token: app.as_ref().map(|app| {
            let app = app.clone();
            AuthorizationTokenSource::provider(move || {
                let app = app.clone();
                async move { app.translator_token() }
            })
        }),
        ..ClientConfig::default()
    };
    let endpoint = Endpoint::<QUIC>::create_client(&config)?;
    let session = Arc::new(endpoint.connect(&args.relay).await?.await?);
    tracing::info!(relay = %args.relay, "connected to relay");
    session
        .subscriber()
        .subscribe_namespace(namespaces.root().to_string(), NamespaceInterest::Namespaces)
        .await
        .context("subscribe to the broadcasts' namespace")?;
    tracing::info!(
        track_namespace_prefix = namespaces.root(),
        "waiting for broadcasts"
    );
    let mut router = Router {
        session,
        namespaces,
        engine,
        capacity: Arc::new(Semaphore::new(args.max_sessions)),
        policy,
        broadcasts: HashMap::new(),
    };
    router.route_events().await
}

fn app_access(args: &Args) -> Option<Arc<AppAccess>> {
    args.app_id
        .clone()
        .zip(args.app_secret.clone())
        .map(|(app_id, secret)| Arc::new(AppAccess::new(app_id, secret)))
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
        None => GeminiCredentials::MetadataServer(MetadataToken::new(http_client::client())),
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

impl Router {
    async fn route_events(&mut self) -> Result<()> {
        loop {
            match self.session.receive_event().await? {
                SessionEvent::PublishNamespace(handler) => {
                    let namespace = handler.track_namespace.clone();
                    if let Err(error) = handler.ok().await {
                        tracing::warn!(%namespace, ?error, "answer PUBLISH_NAMESPACE");
                    }
                    self.start_broadcast(namespace);
                }
                SessionEvent::PublishNamespaceDone(handler) => {
                    self.stop_broadcast(handler.track_namespace());
                }
                SessionEvent::Subscribe(handler) => self.route_subscribe(handler).await,
                SessionEvent::Unsubscribe(handler) => {
                    for broadcast in self.broadcasts.values() {
                        broadcast.unsubscribe(handler.subscribe_id());
                    }
                }
                SessionEvent::Disconnected() => bail!("relay closed the session"),
                SessionEvent::ProtocolViolation() => bail!("relay reported a protocol violation"),
                _ => {}
            }
        }
    }

    fn start_broadcast(&mut self, namespace: String) {
        if !self.namespaces.is_broadcast(&namespace) || self.broadcasts.contains_key(&namespace) {
            return;
        }
        tracing::info!(%namespace, "broadcast started");
        let broadcast = BroadcastTranslation::run(
            self.session.clone(),
            self.engine.clone(),
            self.capacity.clone(),
            self.policy.clone(),
            namespace.clone(),
        );
        self.broadcasts.insert(namespace, broadcast);
    }

    fn stop_broadcast(&mut self, namespace: &str) {
        if self.broadcasts.remove(namespace).is_some() {
            tracing::info!(%namespace, "broadcast ended");
        }
    }

    async fn route_subscribe(&mut self, handler: SubscribeHandler) {
        let target = self
            .namespaces
            .broadcast_of_translated(&handler.track_namespace)
            .filter(|namespace| self.broadcasts.contains_key(*namespace))
            .map(str::to_string);
        let Some(namespace) = target else {
            if let Err(error) = handler
                .error(
                    RequestErrorCode::TrackDoesNotExist,
                    "not a translated namespace".to_string(),
                )
                .await
            {
                tracing::warn!(?error, "send SUBSCRIBE_ERROR");
            }
            return;
        };
        if let Some(broadcast) = self.broadcasts.get(&namespace) {
            broadcast.subscribe(handler);
        }
    }
}
