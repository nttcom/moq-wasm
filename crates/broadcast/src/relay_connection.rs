use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use moqt::{
    ClientConfig, Endpoint, RequestErrorCode, Session, SessionEvent, TerminationErrorCode,
    TransportStats,
    executor::{self, JoinHandle},
    publish::{GroupBoundary, Origin, OutgoingObject, Retention, WriteOutcome},
    subscribe::SubscribeHandler,
};
use msf::CATALOG_TRACK_NAME;
use tokio::sync::{Mutex, watch};

use crate::{
    broadcast::{
        AudioTrackInfo, CatalogMetadata, VideoTrackInfo, awaits_subscribe, build_catalog, is_cmaf,
    },
    stream_ledger::{StreamLedger, StreamRecord},
};

/// Matches the relay's default RELAY_CACHE_TTL_SECS, so a FETCH the relay
/// forwards for a range it never cached can still be answered here.
const FETCH_CACHE_RETENTION: Duration = Duration::from_secs(60);
/// Half the relay's default RELAY_CACHE_TTL_SECS, so the catalog is republished
/// before it falls out of the relay cache (draft-ietf-moq-msf-01 §5) and a viewer
/// arriving long after the last change still finds one.
const CATALOG_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

/// A browser reaches the relay over WebTransport only.
#[cfg(not(target_arch = "wasm32"))]
type RelayTransport = moqt::DUAL;
#[cfg(target_arch = "wasm32")]
type RelayTransport = moqt::BROWSER;

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

/// How long a session a GOAWAY moved away from stays open for tracks whose
/// group started there and has not ended.
const PREVIOUS_SESSION_GRACE: Duration = Duration::from_secs(5);

#[derive(Default)]
struct ConnectionState {
    connected: Option<Arc<ConnectedRelay>>,
    /// The session a GOAWAY moved away from (draft-14 §8.4.2): a track keeps
    /// writing its open group there and starts its next group on `connected`.
    previous: Option<PreviousRelay>,
    migrating: bool,
}

struct PreviousRelay {
    relay: Arc<ConnectedRelay>,
    /// Tracks whose open group is still written to `relay`.
    open_tracks: HashSet<TrackKey>,
    /// `executor::now()` after which `relay` is closed whatever is still open.
    close_at: Duration,
}

type TrackKey = (String, String);

#[derive(Default)]
struct RelayState {
    announced_namespaces: HashSet<String>,
    catalogs: HashMap<String, CatalogMetadata>,
    /// `executor::now()` at the last send.
    catalog_sent_at: HashMap<String, Duration>,
    /// Per namespace path, the tracks a SUBSCRIBE is accepted for before the
    /// catalog lists them.
    declared_tracks: HashMap<String, HashSet<String>>,
    /// Whether a SUBSCRIBE in the namespace has been accepted. Dropped on
    /// disconnect, which ends every wait for one.
    subscribed: HashMap<String, watch::Sender<bool>>,
    /// Tracks an object was written to on this session.
    written_tracks: HashSet<TrackKey>,
    disconnected: bool,
    /// The New Session URI of a GOAWAY (draft-14 §9.4) not acted on yet;
    /// empty means the current URL.
    go_away: Option<String>,
}

impl RelayState {
    fn disconnect(&mut self) {
        self.disconnected = true;
        self.subscribed.clear();
    }

    fn subscribed_sender(&mut self, namespace_path: &str) -> &watch::Sender<bool> {
        self.subscribed
            .entry(namespace_path.to_string())
            .or_insert_with(|| watch::channel(false).0)
    }

    /// The catalog once the announce has started, the declared tracks, and
    /// the CMAF tracks the catalog lists.
    fn subscribable(&self, namespace_path: &str, track_name: &str) -> bool {
        if track_name == CATALOG_TRACK_NAME {
            return self.catalogs.contains_key(namespace_path);
        }
        if self
            .declared_tracks
            .get(namespace_path)
            .is_some_and(|tracks| tracks.contains(track_name))
        {
            return true;
        }
        self.catalogs
            .get(namespace_path)
            .and_then(|metadata| build_catalog(namespace_path, metadata).ok())
            .is_some_and(|catalog| awaits_subscribe(&catalog, track_name))
    }

    fn catalog_refresh_due(&self, namespace_path: &str, now: Duration) -> bool {
        self.catalog_sent_at
            .get(namespace_path)
            .is_some_and(|sent_at| now.saturating_sub(*sent_at) >= CATALOG_REFRESH_INTERVAL)
    }
}

pub(crate) struct ConnectedRelay {
    endpoint: Endpoint<RelayTransport>,
    session: Arc<Session>,
    origin: Origin,
    state: Arc<Mutex<RelayState>>,
    ledger: StreamLedger,
    event_task: JoinHandle,
}

impl RelayConnection {
    // The browser session is !Send, but wasm32 runs on one thread; natively
    // the Arc is shared across tokio worker threads.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
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
        let (connected, previous) = {
            let mut guard = self.inner.lock().await;
            (guard.connected.clone(), guard.previous.take())
        };
        if let Some(previous) = previous {
            previous.relay.close().await;
        }
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

    /// The relay session to send on, connected on first use. After a GOAWAY
    /// the move to the relay it names (draft-14 §8.4.2) runs in the
    /// background while this keeps returning the current session; when the
    /// new relay cannot be reached, the current session is kept until the
    /// relay closes it.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub(crate) async fn connected(&self) -> Result<Option<Arc<ConnectedRelay>>> {
        let Some(target) = &self.target else {
            return Ok(None);
        };
        let mut guard = self.inner.lock().await;
        let current = match &guard.connected {
            Some(current) => current.clone(),
            None => {
                let relay = Arc::new(ConnectedRelay::connect(target, self.ledger.clone()).await?);
                guard.connected = Some(relay.clone());
                relay
            }
        };
        if !guard.migrating
            && let Some(new_session_uri) = current.take_go_away().await
        {
            guard.migrating = true;
            self.spawn_migration(target.clone(), current.clone(), new_session_uri);
        }
        Ok(Some(current))
    }

    /// Writes to the session that carries the track's open group: after a
    /// move, an object within a group the previous session started still
    /// goes there, and the track's next group starts on the new session.
    pub(crate) async fn send_object(
        &self,
        namespace: &[String],
        track_name: &str,
        object: OutgoingObject,
    ) -> Result<bool> {
        let Some(current) = self.connected().await? else {
            return Ok(false);
        };
        let key = (namespace.join("/"), track_name.to_string());
        let Some(previous) = self.previous_for(&key, &object.group).await else {
            return current.send_object(namespace, track_name, object).await;
        };
        match previous.send_object(namespace, track_name, object).await {
            Ok(sent) => Ok(sent),
            Err(err) => {
                tracing::debug!(
                    ?err,
                    "the previous relay session ended before the track moved"
                );
                self.retire_previous().await;
                Ok(false)
            }
        }
    }

    async fn previous_for(
        &self,
        key: &TrackKey,
        group: &GroupBoundary,
    ) -> Option<Arc<ConnectedRelay>> {
        let mut guard = self.inner.lock().await;
        let previous = guard.previous.as_mut()?;
        let route = if matches!(group, GroupBoundary::Within)
            && previous.open_tracks.contains(key)
            && executor::now() < previous.close_at
        {
            Some(previous.relay.clone())
        } else {
            previous.open_tracks.remove(key);
            None
        };
        if previous.open_tracks.is_empty() || executor::now() >= previous.close_at {
            drop(guard);
            self.retire_previous().await;
        }
        route
    }

    async fn retire_previous(&self) {
        let Some(previous) = self.inner.lock().await.previous.take() else {
            return;
        };
        let _ = executor::spawn("broadcast-relay-close", async move {
            previous.relay.close().await
        });
    }

    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    fn spawn_migration(
        &self,
        target: MoqtTarget,
        current: Arc<ConnectedRelay>,
        new_session_uri: String,
    ) {
        let inner = self.inner.clone();
        let ledger = self.ledger.clone();
        let _ = executor::spawn("broadcast-relay-migration", async move {
            let next = current.migrate(&target, &new_session_uri, ledger).await;
            let mut guard = inner.lock().await;
            guard.migrating = false;
            // Catalog changes made on the current session while connecting.
            let next = match next {
                Ok(next) => next.adopt_state(&current).await.map(|()| next),
                Err(err) => Err(err),
            };
            let next = match next {
                Ok(next) => next,
                Err(err) => {
                    tracing::warn!(?err, %new_session_uri, "failed to move to the relay named by GOAWAY; staying on the current session");
                    return;
                }
            };
            let open_tracks = current.state.lock().await.written_tracks.clone();
            if let Some(older) = guard.previous.take() {
                let _ =
                    executor::spawn(
                        "broadcast-relay-close",
                        async move { older.relay.close().await },
                    );
            }
            guard.connected = Some(Arc::new(next));
            guard.previous = Some(PreviousRelay {
                relay: current,
                open_tracks,
                close_at: executor::now() + PREVIOUS_SESSION_GRACE,
            });
            tracing::info!(%new_session_uri, "moved the broadcast to the relay named by GOAWAY");
        });
    }
}

impl ConnectedRelay {
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    async fn connect(target: &MoqtTarget, ledger: StreamLedger) -> Result<Self> {
        let endpoint = Endpoint::<RelayTransport>::create_client(&ClientConfig {
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

    async fn take_go_away(&self) -> Option<String> {
        self.state.lock().await.go_away.take()
    }

    /// Connects to the relay a GOAWAY named and gives it what this session
    /// holds: the announced namespaces, the published tracks and the
    /// catalogs. The new session's `Origin` starts empty, so the FETCH cache
    /// is not carried over and each track starts at its next group.
    async fn migrate(
        &self,
        target: &MoqtTarget,
        new_session_uri: &str,
        ledger: StreamLedger,
    ) -> Result<Self> {
        let url = if new_session_uri.is_empty() {
            target.url.clone()
        } else {
            new_session_uri.to_string()
        };
        let next = Self::connect(
            &MoqtTarget {
                url,
                ..target.clone()
            },
            ledger,
        )
        .await?;
        next.adopt_state(self).await?;
        Ok(next)
    }

    /// Takes over what `from` announced and published; a catalog is sent
    /// again only when it differs from what this session already sent. The
    /// subscriber watches move as well, so a source waiting for its first
    /// subscriber is woken by the new relay's SUBSCRIBE instead of failing
    /// when the old session closes.
    async fn adopt_state(&self, from: &ConnectedRelay) -> Result<()> {
        let (catalogs, sent_namespaces, declared_tracks, subscribed) = {
            let mut guard = from.state.lock().await;
            (
                guard.catalogs.clone(),
                guard
                    .catalog_sent_at
                    .keys()
                    .cloned()
                    .collect::<HashSet<_>>(),
                guard.declared_tracks.clone(),
                std::mem::take(&mut guard.subscribed),
            )
        };
        {
            let mut state = self.state.lock().await;
            for (namespace_path, tracks) in declared_tracks {
                state
                    .declared_tracks
                    .entry(namespace_path)
                    .or_default()
                    .extend(tracks);
            }
            state.subscribed.extend(subscribed);
        }
        for (namespace_path, metadata) in catalogs {
            let namespace: Vec<String> = namespace_path.split('/').map(str::to_string).collect();
            self.announce(&namespace).await?;
            let send = {
                let mut state = self.state.lock().await;
                let changed = state.catalogs.get(&namespace_path) != Some(&metadata);
                let never_sent = !state.catalog_sent_at.contains_key(&namespace_path);
                state.catalogs.insert(namespace_path.clone(), metadata);
                sent_namespaces.contains(&namespace_path) && (changed || never_sent)
            };
            if send {
                self.send_catalog(&namespace_path).await?;
            }
        }
        Ok(())
    }

    fn spawn_event_loop(
        session: Arc<Session>,
        origin: Origin,
        state: Arc<Mutex<RelayState>>,
        ledger: StreamLedger,
    ) -> JoinHandle {
        executor::spawn("broadcast-relay-events", async move {
            loop {
                let event = match session.receive_event().await {
                    Ok(event) => event,
                    Err(err) => {
                        tracing::warn!(?err, "MoQT event loop ended");
                        state.lock().await.disconnect();
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
                        state.lock().await.disconnect();
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
                    SessionEvent::GoAway(handler) => {
                        tracing::info!(
                            new_session_uri = handler.new_session_uri(),
                            "GOAWAY received; moving on the next send"
                        );
                        state.lock().await.go_away = Some(handler.new_session_uri().to_string());
                    }
                    event @ (SessionEvent::MaxRequestId(_)
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
        if self
            .state
            .lock()
            .await
            .catalog_refresh_due(&key.0, executor::now())
        {
            self.send_catalog(&key.0).await?;
        }
        if self.state.lock().await.disconnected {
            bail!("MoQ publisher disconnected");
        }
        ensure_origin_track(&self.origin, &key).await;
        self.state.lock().await.written_tracks.insert(key.clone());
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
        let subscribable = state.lock().await.subscribable(&namespace, &track_name);
        if !subscribable {
            if let Err(err) = handler
                .error(
                    RequestErrorCode::TrackDoesNotExist,
                    format!("{track_name} is neither the catalog nor a CMAF track of it"),
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
            return;
        }
        state
            .lock()
            .await
            .subscribed_sender(&namespace)
            .send_replace(true);
    }

    /// Registers the tracks before the announce, after which the relay may
    /// forward a SUBSCRIBE for them at once.
    pub(crate) async fn declare_tracks(
        &self,
        namespace: &[String],
        track_names: &[String],
    ) -> Result<()> {
        self.state
            .lock()
            .await
            .declared_tracks
            .entry(namespace.join("/"))
            .or_default()
            .extend(track_names.iter().cloned());
        self.announce(namespace).await
    }

    pub(crate) async fn wait_for_subscriber(&self, namespace: &[String]) -> Result<()> {
        let mut subscribed = {
            let mut guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            guard.subscribed_sender(&namespace.join("/")).subscribe()
        };
        subscribed
            .wait_for(|subscribed| *subscribed)
            .await
            .context("MoQ publisher disconnected")?;
        Ok(())
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
        track_name: &str,
        info: AudioTrackInfo,
    ) -> Result<()> {
        self.announce(namespace).await?;
        let namespace_path = namespace.join("/");

        {
            let mut guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            let metadata = guard.catalogs.entry(namespace_path.clone()).or_default();
            if metadata.audio_tracks.get(track_name) == Some(&info) {
                return Ok(());
            }
            metadata.audio_tracks.insert(track_name.to_string(), info);
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
            .insert(namespace_path.to_string(), executor::now());
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
        .ensure_track(
            &key.0,
            &key.1,
            executor::system_time().as_micros() as u64,
            retention,
        )
        .await;
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use moqt::{DUAL, server::ServerConfig, subscribe::FilterType};
    use rcgen::{CertifiedKey, generate_simple_self_signed};
    use tokio::sync::{mpsc, oneshot};

    use super::*;
    use crate::broadcast::{Broadcast, VIDEO_TRACK_NAME, VideoTrackInfo};

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    fn self_signed_server_config(name: &str) -> ServerConfig {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
                .unwrap();
        let dir: PathBuf =
            std::env::temp_dir().join(format!("broadcast-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        std::fs::write(&cert_path, cert.pem()).unwrap();
        std::fs::write(&key_path, signing_key.serialize_pem()).unwrap();
        ServerConfig {
            port: 0,
            cert_path: cert_path.to_string_lossy().into_owned(),
            key_path: key_path.to_string_lossy().into_owned(),
            keep_alive_interval_sec: 5,
        }
    }

    /// A relay that accepts one publisher session, answers its
    /// PUBLISH_NAMESPACE and PUBLISH, and reports every request it received.
    struct FakeRelay {
        url: String,
        session: oneshot::Receiver<Arc<Session>>,
        requests: mpsc::UnboundedReceiver<String>,
    }

    impl FakeRelay {
        fn spawn(name: &str) -> Self {
            Self::spawn_gated(name, None)
        }

        /// PUBLISH_NAMESPACE is answered only once `gate` fires, which holds
        /// a publisher moving here in the middle of the move.
        fn spawn_gated(name: &str, gate: Option<oneshot::Receiver<()>>) -> Self {
            let endpoint =
                Endpoint::<DUAL>::create_server(&self_signed_server_config(name)).unwrap();
            let url = format!("moqt://127.0.0.1:{}", endpoint.local_port().unwrap());
            let (session_sender, session) = oneshot::channel();
            let (request_sender, requests) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                let mut gate = gate;
                let handshake = endpoint.accept().await.unwrap().await.unwrap();
                let session = Arc::new(handshake.accept().await.unwrap());
                let _ = session_sender.send(session.clone());
                while let Ok(event) = session.receive_event().await {
                    match event {
                        SessionEvent::PublishNamespace(handler) => {
                            let _ = request_sender
                                .send(format!("PUBLISH_NAMESPACE {}", handler.track_namespace));
                            if let Some(gate) = gate.take() {
                                let _ = gate.await;
                            }
                            handler.ok().await.unwrap();
                        }
                        SessionEvent::Publish(handler) => {
                            let _ = request_sender.send(format!(
                                "PUBLISH {}/{}",
                                handler.track_namespace, handler.track_name
                            ));
                            handler.ok(0, FilterType::LargestObject, 0).await.unwrap();
                        }
                        SessionEvent::Disconnected() | SessionEvent::ProtocolViolation() => break,
                        _ => {}
                    }
                }
            });
            Self {
                url,
                session,
                requests,
            }
        }

        async fn session(&mut self) -> Arc<Session> {
            tokio::time::timeout(TEST_TIMEOUT, &mut self.session)
                .await
                .expect("the publisher must connect")
                .unwrap()
        }

        fn received_requests(&mut self) -> Vec<String> {
            let mut requests = Vec::new();
            while let Ok(request) = self.requests.try_recv() {
                requests.push(request);
            }
            requests
        }
    }

    fn video_track_info() -> VideoTrackInfo {
        VideoTrackInfo {
            label: "Video".to_string(),
            codec: "avc1.42e01e".to_string(),
            width: 640,
            height: 360,
            init_segment: String::new(),
        }
    }

    fn keyframe_object(group_id: u64) -> OutgoingObject {
        OutgoingObject::plain(GroupBoundary::At(group_id), Bytes::from_static(b"keyframe"))
    }

    fn delta_object() -> OutgoingObject {
        OutgoingObject::plain(GroupBoundary::Within, Bytes::from_static(b"delta"))
    }

    async fn connected_broadcast(relay: &FakeRelay) -> (RelayConnection, Broadcast) {
        let connection = RelayConnection::new(Some(MoqtTarget {
            url: relay.url.clone(),
            auth_token: None,
            verify_certificate: false,
        }));
        let broadcast = Broadcast::new(
            connection.clone(),
            vec!["live".to_string(), "cam".to_string()],
        );
        broadcast
            .update_video_catalog(VIDEO_TRACK_NAME, video_track_info())
            .await
            .unwrap();
        (connection, broadcast)
    }

    async fn wait_until_moved(connection: &RelayConnection) {
        tokio::time::timeout(TEST_TIMEOUT, async {
            while connection.inner.lock().await.previous.is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the broadcast must move to relay B");
    }

    #[tokio::test]
    async fn go_away_moves_the_broadcast_to_the_named_relay_and_closes_the_old_session() {
        // Arrange: the broadcast announces and publishes its catalog on relay A
        let mut relay_a = FakeRelay::spawn("goaway-a");
        let mut relay_b = FakeRelay::spawn("goaway-b");
        let connection = RelayConnection::new(Some(MoqtTarget {
            url: relay_a.url.clone(),
            auth_token: None,
            verify_certificate: false,
        }));
        let broadcast = Broadcast::new(connection, vec!["live".to_string(), "cam".to_string()]);
        broadcast
            .update_video_catalog(VIDEO_TRACK_NAME, video_track_info())
            .await
            .unwrap();
        let session_a = relay_a.session().await;
        assert!(
            relay_a
                .received_requests()
                .contains(&"PUBLISH_NAMESPACE live/cam".to_string())
        );

        // Act: relay A sends GOAWAY naming relay B; the next sends move over
        session_a.go_away(relay_b.url.clone()).await.unwrap();
        let moved = tokio::time::timeout(TEST_TIMEOUT, async {
            let mut group_id = 1;
            loop {
                broadcast
                    .send_object(VIDEO_TRACK_NAME, keyframe_object(group_id))
                    .await
                    .unwrap();
                group_id += 1;
                if relay_b.session.try_recv().is_ok() || !relay_b.requests.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;

        // Assert
        assert!(moved.is_ok(), "the broadcast must move to relay B");
        let requests_b = relay_b.received_requests();
        assert!(
            requests_b.contains(&"PUBLISH_NAMESPACE live/cam".to_string()),
            "{requests_b:?}"
        );
        assert!(
            requests_b.contains(&"PUBLISH live/cam/video".to_string()),
            "{requests_b:?}"
        );
        assert!(
            requests_b.contains(&format!("PUBLISH live/cam/{CATALOG_TRACK_NAME}")),
            "{requests_b:?}"
        );
        let close = tokio::time::timeout(TEST_TIMEOUT, session_a.closed())
            .await
            .expect("the old session must be closed after the move");
        assert_eq!(close.code, Some(0x0));
    }

    #[tokio::test]
    async fn go_away_with_an_unreachable_relay_keeps_the_current_session() {
        // Arrange
        let mut relay_a = FakeRelay::spawn("goaway-unreachable");
        let connection = RelayConnection::new(Some(MoqtTarget {
            url: relay_a.url.clone(),
            auth_token: None,
            verify_certificate: false,
        }));
        let broadcast = Broadcast::new(connection, vec!["live".to_string()]);
        broadcast.announce().await.unwrap();
        let session_a = relay_a.session().await;

        // Act: the named relay does not exist
        session_a
            .go_away("ftp://relay.invalid".to_string())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let sent = broadcast
            .send_object(VIDEO_TRACK_NAME, keyframe_object(1))
            .await;

        // Assert: the send still goes to relay A, which was not closed
        assert!(sent.is_ok());
        assert!(
            tokio::time::timeout(Duration::from_millis(200), session_a.closed())
                .await
                .is_err()
        );
    }

    #[test]
    fn transport_stats_is_none_before_the_relay_session_exists() {
        // Arrange
        let connection = RelayConnection::new(None);

        // Act / Assert
        assert_eq!(connection.transport_stats(), None);
    }

    #[test]
    fn catalog_is_due_again_once_the_refresh_interval_has_passed() {
        // Arrange
        let sent_at = Duration::from_secs(100);
        let mut state = RelayState::default();
        state
            .catalog_sent_at
            .insert("live/test".to_string(), sent_at);

        // Act / Assert
        let almost = sent_at + CATALOG_REFRESH_INTERVAL - Duration::from_secs(1);
        assert!(!state.catalog_refresh_due("live/test", almost));
        let due = sent_at + CATALOG_REFRESH_INTERVAL;
        assert!(state.catalog_refresh_due("live/test", due));
        assert!(!state.catalog_refresh_due("live/other", due));
    }

    #[test]
    fn the_catalog_is_subscribable_from_the_announce_on() {
        // Arrange
        let mut state = RelayState::default();
        state
            .catalogs
            .insert("live/test".to_string(), CatalogMetadata::default());

        // Act / Assert
        assert!(state.subscribable("live/test", CATALOG_TRACK_NAME));
        assert!(!state.subscribable("live/other", CATALOG_TRACK_NAME));
        assert!(!state.subscribable("live/test", "video"));
    }

    #[test]
    fn a_declared_track_is_subscribable_before_the_catalog_lists_it() {
        // Arrange
        let mut state = RelayState::default();
        state
            .declared_tracks
            .entry("onvif/client".to_string())
            .or_default()
            .insert("video/profile_1".to_string());

        // Act / Assert
        assert!(state.subscribable("onvif/client", "video/profile_1"));
        assert!(!state.subscribable("onvif/client", "video/profile_2"));
    }

    #[tokio::test]
    async fn a_wait_for_a_subscriber_ends_on_subscribe_or_disconnect() {
        // Arrange
        let mut state = RelayState::default();
        let mut subscribed = state.subscribed_sender("live/test").subscribe();
        let mut abandoned = state.subscribed_sender("live/other").subscribe();

        // Act
        state.subscribed_sender("live/test").send_replace(true);
        state.disconnect();

        // Assert
        assert!(subscribed.wait_for(|subscribed| *subscribed).await.is_ok());
        assert!(abandoned.wait_for(|subscribed| *subscribed).await.is_err());
    }

    #[tokio::test]
    async fn sends_keep_going_to_the_old_relay_while_the_new_one_is_slow() {
        // Arrange: relay B holds its PUBLISH_NAMESPACE answer
        let mut relay_a = FakeRelay::spawn("goaway-slow-a");
        let (release_b, gate) = oneshot::channel();
        let relay_b = FakeRelay::spawn_gated("goaway-slow-b", Some(gate));
        let (connection, broadcast) = connected_broadcast(&relay_a).await;
        broadcast
            .send_object(VIDEO_TRACK_NAME, keyframe_object(1))
            .await
            .unwrap();
        let session_a = relay_a.session().await;
        session_a.go_away(relay_b.url.clone()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Act
        let sent = tokio::time::timeout(
            Duration::from_secs(1),
            broadcast.send_object(VIDEO_TRACK_NAME, delta_object()),
        )
        .await;

        // Assert: the send did not wait for relay B, and nothing moved yet
        assert!(sent.is_ok(), "a send must not wait for the move");
        assert!(connection.inner.lock().await.previous.is_none());
        let _ = release_b.send(());
        wait_until_moved(&connection).await;
    }

    #[tokio::test]
    async fn the_open_group_ends_on_the_old_relay_and_the_next_one_starts_on_the_new() {
        // Arrange: a group is open on relay A when the move completes
        let mut relay_a = FakeRelay::spawn("goaway-group-a");
        let relay_b = FakeRelay::spawn("goaway-group-b");
        let (connection, broadcast) = connected_broadcast(&relay_a).await;
        broadcast
            .send_object(VIDEO_TRACK_NAME, keyframe_object(1))
            .await
            .unwrap();
        let session_a = relay_a.session().await;
        session_a.go_away(relay_b.url.clone()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        broadcast
            .send_object(VIDEO_TRACK_NAME, delta_object())
            .await
            .unwrap();
        wait_until_moved(&connection).await;

        // Act: one more object of the open group, then the next group
        let within = broadcast
            .send_object(VIDEO_TRACK_NAME, delta_object())
            .await
            .unwrap();
        let old_still_open = tokio::time::timeout(Duration::from_millis(200), session_a.closed())
            .await
            .is_err();
        broadcast
            .send_object(VIDEO_TRACK_NAME, keyframe_object(2))
            .await
            .unwrap();

        // Assert: the object within the group was sent on relay A, which
        // stayed open until the track started its next group
        assert!(within, "the object within the open group must be sent");
        assert!(old_still_open, "relay A must stay open for the open group");
        let close = tokio::time::timeout(TEST_TIMEOUT, session_a.closed())
            .await
            .expect("relay A must be closed once the track moved");
        assert_eq!(close.code, Some(0x0));
    }
}
