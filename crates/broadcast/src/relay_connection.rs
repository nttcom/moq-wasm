use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use mediapack::aac::AudioSpecificConfig;
use moqt::{
    ClientConfig, DUAL, Endpoint, Session, SessionEvent, TerminationErrorCode, TransportStats,
    publish::{Origin, OutgoingObject, Retention, WriteOutcome},
    subscribe::SubscribeHandler,
};
use msf::CATALOG_TRACK_NAME;
use tokio::{sync::Mutex, task::JoinHandle, time::Instant};

use crate::{
    broadcast::{
        CatalogMetadata, VideoTrackInfo, awaits_subscribe, build_catalog, is_cmaf, now_unix,
    },
    stream_ledger::{StreamLedger, StreamRecord},
};

/// SUBSCRIBE_ERROR code, draft-ietf-moq-transport-14 §13.1.2.
const SUBSCRIBE_TRACK_DOES_NOT_EXIST: u64 = 0x4;
/// Matches the relay's default RELAY_CACHE_TTL_SECS, so a FETCH the relay
/// forwards for a range it never cached can still be answered here.
const FETCH_CACHE_RETENTION: Duration = Duration::from_secs(60);
/// Half the relay's default RELAY_CACHE_TTL_SECS, so the catalog is republished
/// before it falls out of the relay cache (draft-ietf-moq-msf-01 §5) and a viewer
/// arriving long after the last change still finds one.
const CATALOG_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct MoqtTarget {
    pub url: String,
    pub auth_token: Option<String>,
    pub verify_certificate: bool,
}

/// Connects on first use; without a target nothing is ever sent.
#[derive(Clone)]
pub struct RelayConnection {
    target: Option<MoqtTarget>,
    inner: Arc<Mutex<ConnectionState>>,
    ledger: StreamLedger,
}

#[derive(Default)]
struct ConnectionState {
    connected: Option<Arc<ConnectedRelay>>,
}

type TrackKey = (String, String);

#[derive(Default)]
struct RelayState {
    announced_namespaces: HashSet<String>,
    catalogs: HashMap<String, CatalogMetadata>,
    catalog_sent_at: HashMap<String, Instant>,
    disconnected: bool,
}

impl RelayState {
    fn catalog_refresh_due(&self, namespace_path: &str) -> bool {
        self.catalog_sent_at
            .get(namespace_path)
            .is_some_and(|sent_at| sent_at.elapsed() >= CATALOG_REFRESH_INTERVAL)
    }
}

pub(crate) struct ConnectedRelay {
    endpoint: Endpoint<DUAL>,
    session: Arc<Session>,
    origin: Origin,
    state: Arc<Mutex<RelayState>>,
    ledger: StreamLedger,
    event_task: JoinHandle<()>,
}

impl RelayConnection {
    pub fn new(target: Option<MoqtTarget>) -> Self {
        Self {
            target,
            inner: Arc::new(Mutex::new(ConnectionState::default())),
            ledger: StreamLedger::default(),
        }
    }

    pub fn streams(&self) -> Vec<StreamRecord> {
        self.ledger.snapshot()
    }

    /// Returns once the CONNECTION_CLOSE has been sent; exiting before that
    /// leaves the relay to end the session only after its idle timeout.
    pub async fn close(&self) {
        let connected = self.inner.lock().await.connected.clone();
        if let Some(connected) = connected {
            connected.close().await;
        }
    }

    /// `None` until the relay session exists; the lock is also held for the
    /// whole connect, so a caller during it sees `None` as well.
    pub fn transport_stats(&self) -> Option<TransportStats> {
        let connected = self.inner.try_lock().ok()?.connected.clone()?;
        Some(connected.session.transport_stats())
    }

    pub(crate) async fn connected(&self) -> Result<Option<Arc<ConnectedRelay>>> {
        let Some(target) = &self.target else {
            return Ok(None);
        };
        let mut guard = self.inner.lock().await;
        if guard.connected.is_none() {
            guard.connected = Some(Arc::new(
                ConnectedRelay::connect(target, self.ledger.clone()).await?,
            ));
        }
        Ok(guard.connected.clone())
    }
}

impl ConnectedRelay {
    async fn connect(target: &MoqtTarget, ledger: StreamLedger) -> Result<Self> {
        let endpoint = Endpoint::<DUAL>::create_client(&ClientConfig {
            port: 0,
            verify_certificate: target.verify_certificate,
            authorization_token: target.auth_token.clone(),
        })?;
        let connecting = endpoint
            .connect(&target.url)
            .await
            .context("connect moqt transport")?;
        let session = Arc::new(connecting.await.context("establish moqt session")?);

        let origin = Origin::new(session.clone());
        let state = Arc::new(Mutex::new(RelayState::default()));
        let event_task = Self::spawn_event_loop(
            session.clone(),
            origin.clone(),
            state.clone(),
            ledger.clone(),
        );

        Ok(Self {
            endpoint,
            session,
            origin,
            state,
            ledger,
            event_task,
        })
    }

    async fn close(&self) {
        self.session
            .close_with_error(TerminationErrorCode::NoError, "");
        self.endpoint.wait_idle().await;
    }

    fn spawn_event_loop(
        session: Arc<Session>,
        origin: Origin,
        state: Arc<Mutex<RelayState>>,
        ledger: StreamLedger,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let event = match session.receive_event().await {
                    Ok(event) => event,
                    Err(err) => {
                        tracing::warn!(?err, "MoQT event loop ended");
                        let mut guard = state.lock().await;
                        guard.disconnected = true;
                        break;
                    }
                };

                match event {
                    SessionEvent::PublishNamespace(handler) => {
                        if let Err(err) = handler.ok().await {
                            tracing::warn!(namespace = %handler.track_namespace, ?err, "failed to ack PUBLISH_NAMESPACE");
                        }
                    }
                    SessionEvent::PublishNamespaceDone(handler) => {
                        tracing::info!(namespace = %handler.track_namespace, "PUBLISH_NAMESPACE_DONE received");
                    }
                    SessionEvent::SubscribeNameSpace(handler) => {
                        if let Err(err) = handler.ok().await {
                            tracing::warn!(prefix = %handler.track_namespace_prefix, ?err, "failed to ack SUBSCRIBE_NAMESPACE");
                        }
                    }
                    SessionEvent::Subscribe(handler) => {
                        Self::serve_subscribe(&origin, &state, handler).await;
                    }
                    SessionEvent::Unsubscribe(handler) => {
                        let request_id = handler.subscribe_id();
                        match origin.release(request_id).await {
                            Some((namespace, track_name)) => {
                                ledger.finish(&track_name);
                                tracing::info!(request_id, %namespace, %track_name, "UNSUBSCRIBE received; track released");
                            }
                            None => {
                                tracing::warn!(request_id, "UNSUBSCRIBE for unknown request");
                            }
                        }
                    }
                    SessionEvent::UnsubscribeNamespace(handler) => {
                        tracing::info!(prefix = %handler.track_namespace_prefix(), "UNSUBSCRIBE_NAMESPACE received");
                    }
                    SessionEvent::Disconnected() | SessionEvent::ProtocolViolation() => {
                        let mut guard = state.lock().await;
                        guard.disconnected = true;
                        break;
                    }
                    SessionEvent::Publish(handler) => {
                        tracing::warn!(namespace = %handler.track_namespace, track = %handler.track_name, "unexpected PUBLISH received");
                    }
                    SessionEvent::Fetch(handler) => {
                        let request_id = handler.request_id;
                        if let Err(err) = origin.serve_fetch(handler).await {
                            tracing::warn!(request_id, ?err, "failed to answer FETCH");
                        }
                    }
                    SessionEvent::FetchCancel(handler) => {
                        origin.cancel_fetch(handler.request_id()).await;
                    }
                    SessionEvent::TrackStatus(handler) => {
                        let request_id = handler.request_id();
                        if let Err(err) = origin.answer_track_status(handler).await {
                            tracing::warn!(request_id, ?err, "failed to answer TRACK_STATUS");
                        }
                    }
                    event @ (SessionEvent::GoAway(_)
                    | SessionEvent::MaxRequestId(_)
                    | SessionEvent::RequestsBlocked(_)
                    | SessionEvent::PublishNamespaceCancel(_)
                    | SessionEvent::PublishDone(_)
                    | SessionEvent::SubscribeUpdate(_)) => {
                        tracing::debug!(?event, "unhandled control message");
                    }
                }
            }
        })
    }

    pub(crate) async fn announce(&self, namespace: &[String]) -> Result<()> {
        let namespace_path = namespace.join("/");
        let announce_needed = {
            let mut guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            guard.catalogs.entry(namespace_path.clone()).or_default();
            !guard.announced_namespaces.contains(&namespace_path)
        };

        if announce_needed {
            self.session
                .publisher()
                .publish_namespace(namespace_path.clone())
                .await
                .with_context(|| format!("publish_namespace {}", namespace_path))?;
            let mut guard = self.state.lock().await;
            guard.announced_namespaces.insert(namespace_path.clone());
            tracing::info!(namespace = %namespace_path, "namespace published");
        }
        Ok(())
    }

    pub(crate) async fn send_object(
        &self,
        namespace: &[String],
        track_name: &str,
        object: OutgoingObject,
    ) -> Result<bool> {
        let key = (namespace.join("/"), track_name.to_string());
        if self.state.lock().await.catalog_refresh_due(&key.0) {
            self.send_catalog(&key.0).await?;
        }
        if self.state.lock().await.disconnected {
            bail!("MoQ publisher disconnected");
        }
        ensure_origin_track(&self.origin, &key).await;
        let bytes = object.payload.len();
        match self.origin.write(&key.0, &key.1, object).await? {
            WriteOutcome::Sent(placement) => {
                if placement.starts_group {
                    self.ledger.open(&key.1, placement.location.group_id);
                }
                self.ledger.add_object(&key.1, bytes);
                Ok(true)
            }
            WriteOutcome::Stopped(placement) => {
                if placement.starts_group {
                    self.ledger.open(&key.1, placement.location.group_id);
                }
                self.ledger.finish(&key.1);
                Ok(false)
            }
            WriteOutcome::NotSent => Ok(false),
        }
    }

    async fn serve_subscribe(
        origin: &Origin,
        state: &Mutex<RelayState>,
        handler: SubscribeHandler,
    ) {
        let namespace = handler.track_namespace.clone();
        let track_name = handler.track_name.clone();
        let subscribable = {
            let guard = state.lock().await;
            guard
                .catalogs
                .get(&namespace)
                .and_then(|metadata| build_catalog(&namespace, metadata).ok())
                .is_some_and(|catalog| awaits_subscribe(&catalog, &track_name))
        };
        if !subscribable {
            if let Err(err) = handler
                .error(
                    SUBSCRIBE_TRACK_DOES_NOT_EXIST,
                    format!("{track_name} is not a CMAF track of the catalog"),
                )
                .await
            {
                tracing::warn!(%namespace, %track_name, ?err, "failed to reject SUBSCRIBE");
            }
            return;
        }
        ensure_origin_track(origin, &(namespace.clone(), track_name.clone())).await;
        if let Err(err) = origin.accept_subscribe(handler).await {
            tracing::warn!(%namespace, %track_name, ?err, "failed to accept SUBSCRIBE");
        }
    }

    pub(crate) async fn update_video_catalog(
        &self,
        namespace: &[String],
        track_name: &str,
        info: VideoTrackInfo,
    ) -> Result<()> {
        self.announce(namespace).await?;
        let namespace_path = namespace.join("/");

        {
            let mut guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            let metadata = guard.catalogs.entry(namespace_path.clone()).or_default();
            if metadata.video_tracks.get(track_name) == Some(&info) {
                return Ok(());
            }
            metadata.video_tracks.insert(track_name.to_string(), info);
        }
        self.send_catalog(&namespace_path).await
    }

    pub(crate) async fn update_audio_catalog(
        &self,
        namespace: &[String],
        config: AudioSpecificConfig,
    ) -> Result<()> {
        self.announce(namespace).await?;
        let namespace_path = namespace.join("/");

        {
            let mut guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            let metadata = guard.catalogs.entry(namespace_path.clone()).or_default();
            if metadata.audio_config.as_ref() == Some(&config) {
                return Ok(());
            }
            metadata.audio_config = Some(config);
        }
        self.send_catalog(&namespace_path).await
    }

    /// The namespace is announced first, since the relay routes a PUBLISH by
    /// its namespace. The non-CMAF tracks are published before the catalog is
    /// sent, so a viewer subscribing to a track it read there finds the relay
    /// already ingesting it; the relay would otherwise forward the SUBSCRIBE
    /// here. CMAF tracks
    /// wait for that SUBSCRIBE, so the uplink carries them only while someone
    /// watches them.
    async fn send_catalog(&self, namespace_path: &str) -> Result<()> {
        let catalog = {
            let guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            let metadata = guard
                .catalogs
                .get(namespace_path)
                .cloned()
                .unwrap_or_default();
            build_catalog(namespace_path, &metadata)?
        };
        let published_tracks = catalog
            .tracks
            .iter()
            .flatten()
            .filter(|track| !is_cmaf(track))
            .map(|track| track.name.as_str());
        for track_name in published_tracks.chain([CATALOG_TRACK_NAME]) {
            self.publish_track((namespace_path.to_string(), track_name.to_string()))
                .await?;
        }
        let payload = Bytes::from(serde_json::to_vec(&catalog).context("serialize msf catalog")?);
        tracing::debug!(namespace = %namespace_path, catalog = %String::from_utf8_lossy(&payload), "catalog snapshot");
        self.state
            .lock()
            .await
            .catalog_sent_at
            .insert(namespace_path.to_string(), Instant::now());
        let bytes = payload.len();
        match self
            .origin
            .write_group(namespace_path, CATALOG_TRACK_NAME, payload)
            .await
            .context("send catalog group")?
        {
            WriteOutcome::Sent(placement) => {
                self.ledger
                    .open(CATALOG_TRACK_NAME, placement.location.group_id);
                self.ledger.add_object(CATALOG_TRACK_NAME, bytes);
                self.ledger.finish(CATALOG_TRACK_NAME);
                tracing::info!(namespace = %namespace_path, group_id = placement.location.group_id, "catalog sent");
            }
            WriteOutcome::Stopped(placement) => {
                self.ledger
                    .open(CATALOG_TRACK_NAME, placement.location.group_id);
                self.ledger.finish(CATALOG_TRACK_NAME);
                tracing::info!(namespace = %namespace_path, "subscriber stopped the catalog group");
            }
            WriteOutcome::NotSent => {}
        }
        Ok(())
    }

    async fn publish_track(&self, key: TrackKey) -> Result<()> {
        ensure_origin_track(&self.origin, &key).await;
        self.origin.publish(&key.0, &key.1).await
    }
}

impl Drop for ConnectedRelay {
    fn drop(&mut self) {
        self.event_task.abort();
    }
}

/// The catalog is kept for as long as the publisher runs, because a viewer
/// that joins a subscription the relay already holds can only obtain it by
/// FETCH, however long ago it was published.
async fn ensure_origin_track(origin: &Origin, key: &TrackKey) {
    let retention = if key.1 == CATALOG_TRACK_NAME {
        Retention::NewestOnly
    } else {
        Retention::For(FETCH_CACHE_RETENTION)
    };
    origin
        .ensure_track(&key.0, &key.1, now_unix().as_micros() as u64, retention)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_stats_is_none_before_the_relay_session_exists() {
        // Arrange
        let connection = RelayConnection::new(None);

        // Act / Assert
        assert_eq!(connection.transport_stats(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn catalog_is_due_again_once_the_refresh_interval_has_passed() {
        // Arrange
        let mut state = RelayState::default();
        state
            .catalog_sent_at
            .insert("live/test".to_string(), Instant::now());

        // Act / Assert
        tokio::time::advance(CATALOG_REFRESH_INTERVAL - Duration::from_secs(1)).await;
        assert!(!state.catalog_refresh_due("live/test"));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(state.catalog_refresh_due("live/test"));
        assert!(!state.catalog_refresh_due("live/other"));
    }
}
