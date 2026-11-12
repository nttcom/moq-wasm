use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use moqt::{
    ClientConfig, Endpoint, QUIC, RequestErrorCode, Session, SessionEvent,
    namespace::NamespaceInterest, subscribe::SubscribeHandler,
};
use tokio::{
    sync::Semaphore,
    time::{Instant, timeout_at},
};

use crate::{
    broadcast_namespace::BroadcastNamespaces, broadcast_translation::BroadcastTranslation,
    translation_engine::TranslationEngine, translation_policy::TranslationPolicy,
};

#[derive(Clone)]
pub(crate) struct TranslationResources {
    pub(crate) engine: TranslationEngine,
    /// One permit per Gemini session that may run at once.
    pub(crate) capacity: Arc<Semaphore>,
    pub(crate) policy: Arc<TranslationPolicy>,
}

/// The only reader of `Session::receive_event` on its session.
pub(crate) struct RootTranslation {
    _endpoint: Endpoint<QUIC>,
    session: Arc<Session>,
    namespaces: BroadcastNamespaces,
    resources: TranslationResources,
    broadcasts: HashMap<String, BroadcastTranslation>,
}

impl RootTranslation {
    pub(crate) async fn connect(
        relay: &str,
        config: &ClientConfig,
        namespaces: BroadcastNamespaces,
        resources: TranslationResources,
    ) -> Result<Self> {
        let endpoint = Endpoint::<QUIC>::create_client(config)?;
        let session = Arc::new(endpoint.connect(relay).await?.await?);
        tracing::info!(%relay, "connected to relay");
        session
            .subscriber()
            .subscribe_namespace(namespaces.root().to_string(), NamespaceInterest::Namespaces)
            .await
            .context("subscribe to the broadcasts' namespace")?;
        tracing::info!(
            track_namespace_prefix = namespaces.root(),
            "waiting for broadcasts"
        );
        Ok(Self {
            _endpoint: endpoint,
            session,
            namespaces,
            resources,
            broadcasts: HashMap::new(),
        })
    }

    /// Runs until the session fails. With `idle_timeout` it also returns,
    /// with `Ok`, once no broadcast has been under the root for that long.
    pub(crate) async fn route_events(&mut self, idle_timeout: Option<Duration>) -> Result<()> {
        let mut last_broadcast_at = Instant::now();
        loop {
            let event = match idle_timeout.filter(|_| self.broadcasts.is_empty()) {
                Some(idle_timeout) => {
                    let deadline = last_broadcast_at + idle_timeout;
                    match timeout_at(deadline, self.session.receive_event()).await {
                        Ok(event) => event?,
                        Err(_) => return Ok(()),
                    }
                }
                None => self.session.receive_event().await?,
            };
            match event {
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
            if !self.broadcasts.is_empty() {
                last_broadcast_at = Instant::now();
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
            self.resources.clone(),
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
