mod broadcast_namespace;
mod broadcast_translation;
mod catalog_publication;
mod echo_translation;
mod gemini;
mod language_translation;
mod opus_codec;
mod pcm_framer;
mod source_audio;
mod source_catalog;
mod target_language;
mod translated_audio;
mod translated_catalog;
mod translation_engine;

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result, bail};
use clap::Parser;
use metadata_token::MetadataToken;
use moqt::{
    ClientConfig, Endpoint, QUIC, RequestErrorCode, Session, SessionEvent,
    subscribe::SubscribeHandler,
};
use tokio::sync::Semaphore;
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

use crate::{
    broadcast_namespace::{BROADCAST_PREFIX, broadcast_of_translated, is_broadcast},
    broadcast_translation::BroadcastTranslation,
    gemini::{AgentPlatform, GeminiCredentials},
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

    #[arg(long, env = "LIVE_TRANSLATE_MAX_SESSIONS", default_value_t = 3)]
    max_sessions: usize,
}

struct Router {
    session: Arc<Session>,
    engine: TranslationEngine,
    capacity: Arc<Semaphore>,
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
    let config = ClientConfig {
        port: 0,
        verify_certificate: !args.insecure,
        ..ClientConfig::default()
    };
    let endpoint = Endpoint::<QUIC>::create_client(&config)?;
    let session = Arc::new(endpoint.connect(&args.relay).await?.await?);
    tracing::info!(relay = %args.relay, "connected to relay");
    session
        .subscriber()
        .subscribe_namespace(BROADCAST_PREFIX.to_string())
        .await
        .context("subscribe to the live-translate namespace")?;
    tracing::info!(prefix = BROADCAST_PREFIX, "waiting for broadcasts");
    let mut router = Router {
        session,
        engine,
        capacity: Arc::new(Semaphore::new(args.max_sessions)),
        broadcasts: HashMap::new(),
    };
    router.route_events().await
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
        if !is_broadcast(&namespace) || self.broadcasts.contains_key(&namespace) {
            return;
        }
        tracing::info!(%namespace, "broadcast started");
        let broadcast = BroadcastTranslation::run(
            self.session.clone(),
            self.engine.clone(),
            self.capacity.clone(),
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
        let target = broadcast_of_translated(&handler.track_namespace)
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
