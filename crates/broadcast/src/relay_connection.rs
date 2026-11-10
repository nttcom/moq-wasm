mod catalog_refresh_task;
mod catalog_track;

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use moqt::{
    CertificateVerification, ClientConfig, Endpoint, RequestErrorCode, Session, SessionEvent,
    TransportStats,
    executor::{self, JoinHandle},
    publish::{Origin, OutgoingObject, Retention, WriteOutcome},
    subscribe::SubscribeHandler,
};
use msf::CATALOG_TRACK_NAME;
use tokio::sync::{Mutex, watch};

use crate::{
    broadcast::{CatalogMetadata, awaits_subscribe, build_catalog},
    stream_ledger::{StreamLedger, StreamRecord},
};
use catalog_refresh_task::CatalogRefreshTask;
use catalog_track::CatalogTrack;

/// Matches the relay's default RELAY_CACHE_TTL_SECS, so a FETCH the relay
/// forwards for a range it never cached can still be answered here.
const FETCH_CACHE_RETENTION: Duration = Duration::from_secs(60);

/// A browser reaches the relay over WebTransport only.
#[cfg(not(target_arch = "wasm32"))]
type RelayTransport = moqt::NATIVE;
#[cfg(target_arch = "wasm32")]
type RelayTransport = moqt::BROWSER;

#[derive(Debug, Clone)]
pub struct MoqtTarget {
    pub url: String,
    pub auth_token: Option<String>,
    pub verify_certificate: bool,
}

/// Connects on first use, or publishes on a session it is given; without
/// either nothing is ever sent.
#[derive(Clone)]
pub struct RelayConnection {
    target: Option<MoqtTarget>,
    connected: Arc<Mutex<Option<Arc<ConnectedRelay>>>>,
    ledger: StreamLedger,
}

type TrackKey = (String, String);

#[derive(Default)]
struct RelayState {
    announced_namespaces: HashSet<String>,
    catalogs: HashMap<String, CatalogMetadata>,
    /// Per namespace path whose catalog has been sent, whether it was sent
    /// since the previous `CatalogRefreshTask` tick.
    catalog_sent_since_tick: HashMap<String, bool>,
    /// Per namespace path, the tracks a SUBSCRIBE is accepted for before the
    /// catalog lists them.
    declared_tracks: HashMap<String, HashSet<String>>,
    /// Per namespace path, the tracks PUBLISH has been sent for. The relay
    /// takes them from the PUBLISH, and the origin keeps them after
    /// `remove_track`. `serve_subscribe` rejects a SUBSCRIBE for one that is
    /// not subscribable; the owner would hand it to the origin.
    published_tracks: HashMap<String, HashSet<String>>,
    /// Whether a SUBSCRIBE in the namespace has been accepted. Dropped on
    /// disconnect, which ends every wait for one.
    subscribed: HashMap<String, watch::Sender<bool>>,
    disconnected: bool,
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

    fn publishes(&self, namespace_path: &str) -> bool {
        self.catalogs.contains_key(namespace_path)
            || self.declared_tracks.contains_key(namespace_path)
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

    fn published(&self, namespace_path: &str, track_name: &str) -> bool {
        self.published_tracks
            .get(namespace_path)
            .is_some_and(|tracks| tracks.contains(track_name))
    }

    /// Returns the namespaces whose catalog was not sent since the previous
    /// tick, and marks every namespace unsent for the next tick.
    fn take_catalogs_to_refresh(&mut self) -> Vec<String> {
        self.catalog_sent_since_tick
            .iter_mut()
            .filter_map(|(namespace_path, sent)| {
                (!std::mem::take(sent)).then(|| namespace_path.clone())
            })
            .collect()
    }

    fn sent_catalogs(&self) -> Vec<String> {
        self.catalog_sent_since_tick.keys().cloned().collect()
    }
}

pub(crate) struct ConnectedRelay {
    owned: Option<OwnedSession>,
    session: Arc<Session>,
    origin: Origin,
    state: Arc<Mutex<RelayState>>,
    catalog: Arc<CatalogTrack>,
    _catalog_refresh: CatalogRefreshTask,
    ledger: StreamLedger,
    events: RelayEvents,
}

struct OwnedSession {
    event_task: JoinHandle,
}

impl Drop for OwnedSession {
    fn drop(&mut self) {
        self.event_task.abort();
    }
}

#[derive(Clone)]
struct RelayEvents {
    origin: Origin,
    state: Arc<Mutex<RelayState>>,
    ledger: StreamLedger,
}

impl RelayConnection {
    // The browser session is !Send, but wasm32 runs on one thread; natively
    // the Arc is shared across tokio worker threads.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub fn new(target: Option<MoqtTarget>) -> Self {
        Self {
            target,
            connected: Arc::new(Mutex::new(None)),
            ledger: StreamLedger::default(),
        }
    }

    /// Publishes on `session`, whose owner keeps reading its events and passes
    /// each to `handle` first. The broadcasts write into `origin`, which the
    /// owner may share with tracks of its own. `close` leaves the session open.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub fn with_session(session: Arc<Session>, origin: Origin) -> Self {
        let ledger = StreamLedger::default();
        let relay = ConnectedRelay::new(false, session, origin, ledger.clone());
        Self {
            target: None,
            connected: Arc::new(Mutex::new(Some(Arc::new(relay)))),
            ledger,
        }
    }

    /// Answers SUBSCRIBE, UNSUBSCRIBE, FETCH, FETCH_CANCEL and TRACK_STATUS
    /// for the broadcasts on a session given to `with_session`, and returns
    /// every other event. That includes a SUBSCRIBE in a namespace no
    /// broadcast here publishes, a SUBSCRIBE for a track no broadcast here
    /// has published (the owner may publish it with the `origin` given to
    /// `with_session`), and an UNSUBSCRIBE that this `origin` does not know.
    /// `Disconnected` and `ProtocolViolation` are returned too, after they
    /// ended the broadcasts' sends.
    pub async fn handle(&self, event: SessionEvent) -> Option<SessionEvent> {
        let connected = self.connected.lock().await.clone();
        match connected {
            Some(relay) => relay.events.handle(event).await,
            None => Some(event),
        }
    }

    pub fn streams(&self) -> Vec<StreamRecord> {
        self.ledger.snapshot()
    }

    /// Returns once the CONNECTION_CLOSE has been sent; exiting before that
    /// leaves the relay to end the session only after its idle timeout.
    pub async fn close(&self) {
        let connected = self.connected.lock().await.clone();
        if let Some(connected) = connected
            && connected.owned.is_some()
        {
            connected.session.close().await;
        }
    }

    /// `None` until the relay session exists; the lock is also held for the
    /// whole connect, so a caller during it sees `None` as well.
    pub fn transport_stats(&self) -> Option<TransportStats> {
        let connected = self.connected.try_lock().ok()?.clone()?;
        Some(connected.session.transport_stats())
    }

    /// The relay session to send on, connected on first use; it follows the
    /// relay's GOAWAY (`ClientConfig::follow_go_away`).
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub(crate) async fn connected(&self) -> Result<Option<Arc<ConnectedRelay>>> {
        let mut connected = self.connected.lock().await;
        if let Some(relay) = connected.as_ref() {
            return Ok(Some(relay.clone()));
        }
        let Some(target) = &self.target else {
            return Ok(None);
        };
        let relay = Arc::new(ConnectedRelay::connect(target, self.ledger.clone()).await?);
        *connected = Some(relay.clone());
        Ok(Some(relay))
    }
}

impl ConnectedRelay {
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    async fn connect(target: &MoqtTarget, ledger: StreamLedger) -> Result<Self> {
        let endpoint = Endpoint::<RelayTransport>::create_client(&ClientConfig {
            certificate_verification: CertificateVerification::native_roots_or_disabled(
                target.verify_certificate,
            ),
            authorization_token: target.auth_token.clone().map(Into::into),
            ..ClientConfig::default()
        })?;
        let connecting = endpoint
            .connect(&target.url)
            .await
            .context("connect moqt transport")?;
        let session = Arc::new(connecting.await.context("establish moqt session")?);
        Ok(Self::new(true, session, Origin::default(), ledger))
    }

    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    fn new(
        owns_session: bool,
        session: Arc<Session>,
        origin: Origin,
        ledger: StreamLedger,
    ) -> Self {
        let state = Arc::new(Mutex::new(RelayState::default()));
        let events = RelayEvents {
            origin: origin.clone(),
            state: state.clone(),
            ledger: ledger.clone(),
        };
        let owned = owns_session.then(|| OwnedSession {
            event_task: Self::spawn_event_loop(session.clone(), events.clone()),
        });
        let catalog = Arc::new(CatalogTrack::new(
            session.clone(),
            origin.clone(),
            state.clone(),
            ledger.clone(),
        ));
        Self {
            owned,
            session,
            origin,
            state,
            _catalog_refresh: CatalogRefreshTask::run(catalog.clone()),
            catalog,
            ledger,
            events,
        }
    }

    fn spawn_event_loop(session: Arc<Session>, events: RelayEvents) -> JoinHandle {
        executor::spawn("broadcast-relay-events", async move {
            loop {
                let event = match session.receive_event().await {
                    Ok(event) => event,
                    Err(err) => {
                        tracing::warn!(?err, "MoQT event loop ended");
                        events.state.lock().await.disconnect();
                        break;
                    }
                };
                let Some(event) = events.handle(event).await else {
                    continue;
                };
                match event {
                    SessionEvent::PublishNamespace(handler) => {
                        if let Err(err) = handler.ok().await {
                            tracing::warn!(track_namespace = %handler.track_namespace, ?err, "failed to ack PUBLISH_NAMESPACE");
                        }
                    }
                    SessionEvent::PublishNamespaceDone(handler) => {
                        tracing::info!(track_namespace = %handler.track_namespace, "PUBLISH_NAMESPACE_DONE received");
                    }
                    SessionEvent::SubscribeNameSpace(handler) => {
                        if let Err(err) = handler.ok().await {
                            tracing::warn!(track_namespace_prefix = %handler.track_namespace_prefix, ?err, "failed to ack SUBSCRIBE_NAMESPACE");
                        }
                    }
                    SessionEvent::UnsubscribeNamespace(handler) => {
                        tracing::info!(track_namespace_prefix = %handler.track_namespace_prefix(), "UNSUBSCRIBE_NAMESPACE received");
                    }
                    SessionEvent::Disconnected() | SessionEvent::ProtocolViolation() => break,
                    SessionEvent::Subscribe(handler) => {
                        let reason = format!(
                            "{}/{} is not published here",
                            handler.track_namespace, handler.track_name
                        );
                        if let Err(err) = handler
                            .error(RequestErrorCode::TrackDoesNotExist, reason)
                            .await
                        {
                            tracing::warn!(?err, "failed to reject SUBSCRIBE");
                        }
                    }
                    SessionEvent::Unsubscribe(handler) => {
                        tracing::warn!(
                            request_id = handler.subscribe_id(),
                            "UNSUBSCRIBE for unknown request"
                        );
                    }
                    SessionEvent::Publish(handler) => {
                        tracing::warn!(track_namespace = %handler.track_namespace, track_name = %handler.track_name, "unexpected PUBLISH received");
                    }
                    event => {
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

    /// Returns `handler` for a namespace no broadcast here publishes and for a
    /// track no broadcast here has published.
    async fn serve_subscribe(
        origin: &Origin,
        state: &Mutex<RelayState>,
        handler: SubscribeHandler,
    ) -> Option<SubscribeHandler> {
        let namespace = handler.track_namespace.clone();
        let track_name = handler.track_name.clone();
        let (publishes, subscribable, published) = {
            let state = state.lock().await;
            (
                state.publishes(&namespace),
                state.subscribable(&namespace, &track_name),
                state.published(&namespace, &track_name),
            )
        };
        if !publishes || !(subscribable || published) {
            return Some(handler);
        }
        if !subscribable {
            if let Err(err) = handler
                .error(
                    RequestErrorCode::TrackDoesNotExist,
                    format!("{track_name} is neither the catalog nor a CMAF track of it"),
                )
                .await
            {
                tracing::warn!(track_namespace = %namespace, %track_name, ?err, "failed to reject SUBSCRIBE");
            }
            return None;
        }
        ensure_origin_track(origin, &(namespace.clone(), track_name.clone())).await;
        if let Err(err) = origin.accept_subscribe(handler).await {
            tracing::warn!(track_namespace = %namespace, %track_name, ?err, "failed to accept SUBSCRIBE");
            return None;
        }
        state
            .lock()
            .await
            .subscribed_sender(&namespace)
            .send_replace(true);
        None
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

    pub(crate) async fn update_catalog<T: PartialEq>(
        &self,
        namespace: &[String],
        track_name: &str,
        info: T,
        tracks: fn(&mut CatalogMetadata) -> &mut BTreeMap<String, T>,
    ) -> Result<()> {
        self.update_metadata(namespace, |metadata| {
            let tracks = tracks(metadata);
            if tracks.get(track_name) == Some(&info) {
                return false;
            }
            tracks.insert(track_name.to_string(), info);
            true
        })
        .await
    }

    /// Sends the catalog when `update` reports that it changed the metadata.
    pub(crate) async fn update_metadata(
        &self,
        namespace: &[String],
        update: impl FnOnce(&mut CatalogMetadata) -> bool,
    ) -> Result<()> {
        self.announce(namespace).await?;
        let namespace_path = namespace.join("/");
        {
            let mut guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            if !update(guard.catalogs.entry(namespace_path.clone()).or_default()) {
                return Ok(());
            }
        }
        self.catalog.send(&namespace_path).await
    }

    pub(crate) async fn remove_track(&self, namespace: &[String], track_name: &str) -> Result<()> {
        let namespace_path = namespace.join("/");
        let dropped = {
            let mut guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            let Some(metadata) = guard.catalogs.get_mut(&namespace_path) else {
                return Ok(());
            };
            let before = catalog_track_names(&namespace_path, metadata)?;
            if !metadata.remove_track(track_name) {
                return Ok(());
            }
            let after = catalog_track_names(&namespace_path, metadata)?;
            before.difference(&after).cloned().collect::<Vec<_>>()
        };
        self.catalog.send(&namespace_path).await?;
        for name in dropped {
            self.origin.end_track(&namespace_path, &name).await?;
            self.ledger.finish(&name);
            tracing::info!(track_namespace = %namespace_path, track_name = %name, "track removed");
        }
        Ok(())
    }
}

impl RelayEvents {
    async fn handle(&self, event: SessionEvent) -> Option<SessionEvent> {
        // Answered here instead of by `origin.handle`: a SUBSCRIBE is checked
        // against the catalog and raises the subscribed signal, and an
        // UNSUBSCRIBE finishes the track in the stream ledger.
        match event {
            SessionEvent::Subscribe(handler) => {
                ConnectedRelay::serve_subscribe(&self.origin, &self.state, handler)
                    .await
                    .map(SessionEvent::Subscribe)
            }
            SessionEvent::Unsubscribe(handler) => {
                let request_id = handler.subscribe_id();
                let Some((namespace, track_name)) = self.origin.release(&handler).await else {
                    return Some(SessionEvent::Unsubscribe(handler));
                };
                self.ledger.finish(&track_name);
                tracing::info!(request_id, track_namespace = %namespace, %track_name, "UNSUBSCRIBE received; track released");
                None
            }
            event @ (SessionEvent::Disconnected() | SessionEvent::ProtocolViolation()) => {
                self.state.lock().await.disconnect();
                Some(event)
            }
            event => self.origin.handle(event).await,
        }
    }
}

fn catalog_track_names(
    namespace_path: &str,
    metadata: &CatalogMetadata,
) -> Result<HashSet<String>> {
    Ok(build_catalog(namespace_path, metadata)?
        .tracks
        .into_iter()
        .flatten()
        .map(|track| track.name)
        .collect())
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

    use bytes::Bytes;
    use moqt::{
        ClientConfig, NATIVE, Subscription, TrackObject, TrackReader,
        publish::GroupBoundary,
        server::{CertificateFiles, ServerConfig, write_self_signed_cert},
        subscribe::FilterType,
    };
    use std::collections::BTreeSet;

    use msf::Catalog;
    use tokio::sync::{mpsc, oneshot};

    use super::*;
    use mediapack::AudioConfig;

    use crate::broadcast::{AudioTrackInfo, Broadcast, VIDEO_TRACK_NAME, VideoTrackInfo};

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    fn self_signed_server_config(name: &str) -> ServerConfig {
        let dir: PathBuf =
            std::env::temp_dir().join(format!("broadcast-test-{name}-{}", std::process::id()));
        let CertificateFiles {
            cert_path,
            key_path,
        } = write_self_signed_cert(&dir, &["localhost", "127.0.0.1"]).unwrap();
        ServerConfig {
            port: 0,
            cert_path,
            key_path,
            keep_alive_interval_sec: 5,
            // The tests write and read draft-14 messages.
            versions: vec![moqt::Version::Draft14],
        }
    }

    /// A relay that accepts one publisher session and answers its
    /// PUBLISH_NAMESPACE and PUBLISH.
    struct FakeRelay {
        url: String,
        session: oneshot::Receiver<Arc<Session>>,
        requests: mpsc::UnboundedReceiver<String>,
        published: mpsc::UnboundedReceiver<Subscription>,
        video_objects: mpsc::UnboundedReceiver<TrackObject>,
        catalog_objects: mpsc::UnboundedReceiver<TrackObject>,
    }

    impl FakeRelay {
        fn spawn(name: &str) -> Self {
            Self::spawn_gated(name, None, None)
        }

        /// PUBLISH_NAMESPACE is answered only once `namespace_gate` fires;
        /// the catalog track is read only once `catalog_gate` fires.
        fn spawn_gated(
            name: &str,
            namespace_gate: Option<oneshot::Receiver<()>>,
            catalog_gate: Option<oneshot::Receiver<()>>,
        ) -> Self {
            let endpoint =
                Endpoint::<NATIVE>::create_server(&self_signed_server_config(name)).unwrap();
            let url = format!("moqt://127.0.0.1:{}", endpoint.local_port().unwrap());
            let (session_sender, session) = oneshot::channel();
            let (request_sender, requests) = mpsc::unbounded_channel();
            let (published_sender, published) = mpsc::unbounded_channel();
            let (video_object_sender, video_objects) = mpsc::unbounded_channel();
            let (catalog_object_sender, catalog_objects) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                let (mut namespace_gate, mut catalog_gate) = (namespace_gate, catalog_gate);
                let mut published_tracks = HashMap::new();
                let handshake = endpoint.accept().await.unwrap().await.unwrap();
                let session = Arc::new(handshake.accept(&tracing::Span::current()).await.unwrap());
                let _ = session_sender.send(session.clone());
                while let Ok(event) = session.receive_event().await {
                    match event {
                        SessionEvent::PublishNamespace(handler) => {
                            let _ = request_sender
                                .send(format!("PUBLISH_NAMESPACE {}", handler.track_namespace));
                            // Answered aside, so requests sent meanwhile are still recorded
                            let gate = namespace_gate.take();
                            tokio::spawn(async move {
                                if let Some(gate) = gate {
                                    let _ = gate.await;
                                }
                                handler.ok().await.unwrap();
                            });
                        }
                        SessionEvent::Publish(handler) => {
                            let _ = request_sender.send(format!(
                                "PUBLISH {}/{}",
                                handler.track_namespace, handler.track_name
                            ));
                            let object_sender = match handler.track_name.as_str() {
                                VIDEO_TRACK_NAME => Some(video_object_sender.clone()),
                                CATALOG_TRACK_NAME => Some(catalog_object_sender.clone()),
                                _ => None,
                            };
                            if object_sender.is_some() {
                                handler.accept_data_receiver().await;
                            }
                            let subscription =
                                handler.ok(0, FilterType::LargestObject).await.unwrap();
                            published_tracks.insert(
                                subscription.request_id(),
                                format!("{}/{}", handler.track_namespace, handler.track_name),
                            );
                            if let Some(sender) = object_sender {
                                let session = session.clone();
                                let subscription = subscription.clone();
                                let gate = (handler.track_name == CATALOG_TRACK_NAME)
                                    .then(|| catalog_gate.take())
                                    .flatten();
                                tokio::spawn(async move {
                                    if let Some(gate) = gate {
                                        let _ = gate.await;
                                    }
                                    let Ok(receiver) = session
                                        .subscriber()
                                        .accept_data_receiver(&subscription)
                                        .await
                                    else {
                                        return;
                                    };
                                    let mut reader = TrackReader::new(receiver);
                                    while let Ok(Some(object)) = reader.next_object().await {
                                        let _ = sender.send(object);
                                    }
                                });
                            }
                            let _ = published_sender.send(subscription);
                        }
                        SessionEvent::PublishDone(handler) => {
                            if let Some(track) = published_tracks.get(&handler.request_id()) {
                                let _ = request_sender.send(format!("PUBLISH_DONE {track}"));
                            }
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
                published,
                video_objects,
                catalog_objects,
            }
        }

        /// Mimics a draining relay whose subscribers have left.
        async fn unsubscribe_published(&mut self, session: &Session) {
            while let Ok(subscription) = self.published.try_recv() {
                session
                    .subscriber()
                    .unsubscribe(&subscription)
                    .await
                    .unwrap();
            }
        }

        async fn wait_for_video_group(&mut self, group_id: u64) {
            let mut seen = Vec::new();
            let arrived = tokio::time::timeout(TEST_TIMEOUT, async {
                while let Some(object) = self.video_objects.recv().await {
                    seen.push(object.group_id);
                    if object.group_id == group_id {
                        return;
                    }
                }
            })
            .await;
            assert!(
                arrived.is_ok(),
                "video group {group_id} must arrive at {}, saw {seen:?}",
                self.url
            );
        }

        async fn session(&mut self) -> Arc<Session> {
            tokio::time::timeout(TEST_TIMEOUT, &mut self.session)
                .await
                .expect("the publisher must connect")
                .unwrap()
        }

        async fn wait_for_request(&mut self, expected: &str) {
            let mut seen = Vec::new();
            let arrived = tokio::time::timeout(TEST_TIMEOUT, async {
                while let Some(request) = self.requests.recv().await {
                    let found = request == expected;
                    seen.push(request);
                    if found {
                        return;
                    }
                }
            })
            .await;
            assert!(arrived.is_ok(), "{expected} must arrive, saw {seen:?}");
        }

        async fn received_catalogs(&mut self) -> Vec<TrackObject> {
            let mut catalogs = Vec::new();
            while let Ok(Some(object)) =
                tokio::time::timeout(Duration::from_millis(300), self.catalog_objects.recv()).await
            {
                catalogs.push(object);
            }
            catalogs
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
            bitrate: None,
            cmaf: true,
            switching_set: VIDEO_TRACK_NAME.to_string(),
            source: VIDEO_TRACK_NAME.to_string(),
        }
    }

    fn keyframe_object(group_id: u64) -> OutgoingObject {
        OutgoingObject::plain(GroupBoundary::At(group_id), Bytes::from_static(b"keyframe"))
    }

    fn delta_object() -> OutgoingObject {
        OutgoingObject::plain(GroupBoundary::Within, Bytes::from_static(b"delta"))
    }

    fn connection_to(relay: &FakeRelay) -> RelayConnection {
        RelayConnection::new(Some(MoqtTarget {
            url: relay.url.clone(),
            auth_token: None,
            verify_certificate: false,
        }))
    }

    fn live_cam(connection: RelayConnection) -> Broadcast {
        Broadcast::new(connection, vec!["live".to_string(), "cam".to_string()])
    }

    async fn connected_broadcast(relay: &FakeRelay) -> (RelayConnection, Broadcast) {
        let connection = connection_to(relay);
        let broadcast = live_cam(connection.clone());
        broadcast
            .update_video_catalog(VIDEO_TRACK_NAME, video_track_info())
            .await
            .unwrap();
        (connection, broadcast)
    }

    /// The sleep gives relay B's answer to the PUBLISH time to arrive.
    async fn wait_until_moved(relay_b: &mut FakeRelay) {
        relay_b
            .wait_for_request(&format!("PUBLISH live/cam/{VIDEO_TRACK_NAME}"))
            .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn go_away_moves_the_broadcast_to_the_named_relay_and_closes_the_old_session() {
        // Arrange: the broadcast announces and publishes its catalog on relay A
        let mut relay_a = FakeRelay::spawn("goaway-a");
        let mut relay_b = FakeRelay::spawn("goaway-b");
        let broadcast = live_cam(connection_to(&relay_a));
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

        // Act: relay A sends GOAWAY naming relay B
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
        relay_a.unsubscribe_published(&session_a).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        broadcast
            .send_object(VIDEO_TRACK_NAME, keyframe_object(100))
            .await
            .unwrap();
        let close = tokio::time::timeout(TEST_TIMEOUT, session_a.closed())
            .await
            .expect("the old session must be closed once relay A unsubscribed");
        assert_eq!(close.code, Some(0x0));
    }

    #[tokio::test]
    async fn the_relay_a_go_away_names_gets_the_catalog_while_no_media_flows() {
        // Arrange: the broadcast has sent its catalog to relay A and sends no media
        let mut relay_a = FakeRelay::spawn("goaway-catalog-a");
        let mut relay_b = FakeRelay::spawn("goaway-catalog-b");
        let (_connection, _broadcast) = connected_broadcast(&relay_a).await;
        let session_a = relay_a.session().await;

        // Act
        session_a.go_away(relay_b.url.clone()).await.unwrap();
        let catalog = tokio::time::timeout(TEST_TIMEOUT, relay_b.catalog_objects.recv()).await;

        // Assert
        let catalog = catalog
            .expect("relay B must get the catalog")
            .expect("relay B keeps reading the catalog");
        assert!(serde_json::from_slice::<Catalog>(&catalog.payload).is_ok());
    }

    #[tokio::test]
    async fn a_track_is_published_only_after_the_relay_accepts_its_namespace() {
        // Arrange: the relay holds its PUBLISH_NAMESPACE_OK
        let (accept_namespace, namespace_gate) = oneshot::channel();
        let mut relay =
            FakeRelay::spawn_gated("namespace-before-publish", Some(namespace_gate), None);
        let broadcast = live_cam(connection_to(&relay));
        let catalog_update = tokio::spawn(async move {
            broadcast
                .update_video_catalog(VIDEO_TRACK_NAME, video_track_info())
                .await
        });
        relay.wait_for_request("PUBLISH_NAMESPACE live/cam").await;

        // Act
        tokio::time::sleep(Duration::from_millis(200)).await;
        let before_ok = relay.received_requests();
        let _ = accept_namespace.send(());
        relay
            .wait_for_request(&format!("PUBLISH live/cam/{CATALOG_TRACK_NAME}"))
            .await;

        // Assert
        assert_eq!(before_ok, Vec::<String>::new());
        tokio::time::timeout(TEST_TIMEOUT, catalog_update)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn go_away_with_an_unreachable_relay_keeps_the_current_session() {
        // Arrange
        let mut relay_a = FakeRelay::spawn("goaway-unreachable");
        let broadcast = Broadcast::new(connection_to(&relay_a), vec!["live".to_string()]);
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
    fn a_tick_refreshes_only_the_catalogs_not_sent_since_the_previous_tick() {
        // Arrange
        let mut state = RelayState::default();
        state
            .catalog_sent_since_tick
            .insert("live/changed".to_string(), true);
        state
            .catalog_sent_since_tick
            .insert("live/idle".to_string(), false);

        // Act
        let refreshed = state.take_catalogs_to_refresh();

        // Assert
        assert_eq!(refreshed, vec!["live/idle".to_string()]);
    }

    #[test]
    fn a_catalog_sent_since_one_tick_is_refreshed_at_the_next() {
        // Arrange
        let mut state = RelayState::default();
        state
            .catalog_sent_since_tick
            .insert("live/changed".to_string(), true);
        state.take_catalogs_to_refresh();

        // Act
        let refreshed = state.take_catalogs_to_refresh();

        // Assert
        assert_eq!(refreshed, vec!["live/changed".to_string()]);
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

    /// A broadcast on a session whose owner reads the events: every SUBSCRIBE
    /// the connection returns is sent to `returned` as `namespace/track` and
    /// rejected.
    struct OwnedSessionBroadcast {
        session: Arc<Session>,
        connection: RelayConnection,
        broadcast: Broadcast,
        returned: mpsc::UnboundedReceiver<String>,
        owner_loop: tokio::task::JoinHandle<()>,
    }

    async fn owned_session_broadcast(relay: &FakeRelay) -> OwnedSessionBroadcast {
        let endpoint = Endpoint::<NATIVE>::create_client(&ClientConfig {
            certificate_verification: CertificateVerification::Disabled,
            ..ClientConfig::default()
        })
        .unwrap();
        let session = Arc::new(endpoint.connect(&relay.url).await.unwrap().await.unwrap());
        let connection = RelayConnection::with_session(session.clone(), Origin::default());
        let (returned_sender, returned) = mpsc::unbounded_channel();
        let owner_loop = tokio::spawn({
            let session = session.clone();
            let connection = connection.clone();
            async move {
                while let Ok(event) = session.receive_event().await {
                    if let Some(SessionEvent::Subscribe(handler)) = connection.handle(event).await {
                        let _ = returned_sender.send(format!(
                            "{}/{}",
                            handler.track_namespace, handler.track_name
                        ));
                        let _ = handler
                            .error(RequestErrorCode::TrackDoesNotExist, String::new())
                            .await;
                    }
                }
            }
        });
        let broadcast = Broadcast::new(
            connection.clone(),
            vec!["live".to_string(), "cam".to_string()],
        );
        broadcast
            .update_video_catalog(VIDEO_TRACK_NAME, video_track_info())
            .await
            .unwrap();
        OwnedSessionBroadcast {
            session,
            connection,
            broadcast,
            returned,
            owner_loop,
        }
    }

    async fn relay_subscribe(relay_session: &Session, namespace: &str, track: &str) -> bool {
        let mut subscriber = relay_session.subscriber();
        matches!(
            tokio::time::timeout(
                TEST_TIMEOUT,
                subscriber.subscribe(
                    namespace.to_string(),
                    track.to_string(),
                    moqt::subscribe::SubscribeOption::default(),
                ),
            )
            .await,
            Ok(Ok(_))
        )
    }

    #[tokio::test]
    async fn a_broadcast_publishes_on_a_session_its_owner_reads_and_close_leaves_it_open() {
        // Arrange
        let mut relay = FakeRelay::spawn("shared-session");
        let mut owned = owned_session_broadcast(&relay).await;

        // Act
        let catalogs = relay.received_catalogs().await;
        let relay_session = relay.session().await;
        let subscribed = relay_subscribe(&relay_session, "live/cam", CATALOG_TRACK_NAME).await;
        relay_subscribe(&relay_session, "other/ns", "track").await;
        let returned = owned.returned.try_recv();
        owned.connection.close().await;
        let still_open =
            tokio::time::timeout(Duration::from_millis(300), owned.session.closed()).await;

        // Assert
        assert_eq!(
            relay.received_requests()[..2],
            [
                "PUBLISH_NAMESPACE live/cam".to_string(),
                format!("PUBLISH live/cam/{VIDEO_TRACK_NAME}"),
            ]
        );
        assert!(!catalogs.is_empty(), "the catalog must arrive");
        assert!(subscribed, "the SUBSCRIBE must be answered through handle");
        assert_eq!(
            returned.ok().as_deref(),
            Some("other/ns/track"),
            "a SUBSCRIBE in a namespace no broadcast publishes goes back to the owner"
        );
        assert!(
            still_open.is_err(),
            "close must leave the owner's session open"
        );
        owned.owner_loop.abort();
    }

    #[tokio::test]
    async fn a_subscribe_for_a_track_the_broadcast_never_listed_goes_back_to_the_owner() {
        // Arrange
        let mut relay = FakeRelay::spawn("unlisted-track");
        let mut owned = owned_session_broadcast(&relay).await;
        relay.received_catalogs().await;

        let relay_session = relay.session().await;

        // Act
        relay_subscribe(&relay_session, "live/cam", "prompt").await;

        // Assert
        assert_eq!(
            owned.returned.try_recv().ok().as_deref(),
            Some("live/cam/prompt")
        );
        owned.owner_loop.abort();
    }

    #[tokio::test]
    async fn a_subscribe_for_a_removed_track_is_rejected_without_reaching_the_owner() {
        // Arrange
        let mut relay = FakeRelay::spawn("removed-track");
        let mut owned = owned_session_broadcast(&relay).await;
        relay.received_catalogs().await;
        owned
            .broadcast
            .remove_track(VIDEO_TRACK_NAME)
            .await
            .unwrap();

        let relay_session = relay.session().await;

        // Act
        let subscribed = relay_subscribe(&relay_session, "live/cam", VIDEO_TRACK_NAME).await;

        // Assert
        assert!(!subscribed);
        assert!(owned.returned.try_recv().is_err());
        owned.owner_loop.abort();
    }

    #[tokio::test]
    async fn remove_track_ends_it_with_the_tracks_listed_only_with_it_and_an_update_publishes_it_again()
     {
        // Arrange: the audio placeholder and the chat are listed only because of `video`, and
        // the rendition keeps the source's timeline
        let mut relay = FakeRelay::spawn("remove-track");
        let (_connection, broadcast) = connected_broadcast(&relay).await;
        broadcast
            .update_video_catalog("video_480p", video_track_info())
            .await
            .unwrap();
        relay.wait_for_request("PUBLISH live/cam/video_480p").await;
        relay.received_catalogs().await;

        // Act
        broadcast.remove_track(VIDEO_TRACK_NAME).await.unwrap();
        let ended = tokio::time::timeout(TEST_TIMEOUT, async {
            let mut ended = BTreeSet::new();
            while ended.len() < 3 {
                let request = relay.requests.recv().await.unwrap();
                if let Some(track) = request.strip_prefix("PUBLISH_DONE ") {
                    ended.insert(track.to_string());
                }
            }
            ended
        })
        .await
        .expect("PUBLISH_DONE must arrive for every dropped track");
        let catalogs = relay.received_catalogs().await;
        let after_removal = relay.received_requests();
        broadcast
            .update_video_catalog(VIDEO_TRACK_NAME, video_track_info())
            .await
            .unwrap();

        // Assert
        assert_eq!(
            ended,
            BTreeSet::from(
                ["audio", "chat", VIDEO_TRACK_NAME].map(|name| format!("live/cam/{name}"))
            )
        );
        assert!(
            !after_removal
                .iter()
                .any(|request| request.starts_with("PUBLISH_DONE")),
            "{after_removal:?}"
        );
        let newest: Catalog =
            serde_json::from_slice(&catalogs.last().expect("a catalog must arrive").payload)
                .unwrap();
        let names: Vec<&str> = newest
            .tracks
            .iter()
            .flatten()
            .map(|track| track.name.as_str())
            .collect();
        assert_eq!(names, ["video_480p", "video_480p_cmaf", "video_timeline"]);
        relay
            .wait_for_request(&format!("PUBLISH live/cam/{VIDEO_TRACK_NAME}"))
            .await;
    }

    #[tokio::test]
    async fn removing_the_default_audio_ends_it_without_a_placeholder_while_video_stays() {
        // Arrange
        let mut relay = FakeRelay::spawn("remove-default-audio");
        let (_connection, broadcast) = connected_broadcast(&relay).await;
        broadcast
            .update_audio_catalog(
                "audio",
                AudioTrackInfo {
                    label: "Audio".to_string(),
                    config: AudioConfig::Opus {
                        sample_rate: 48_000,
                        channels: 1,
                    },
                    cmaf: false,
                    switching_set: VIDEO_TRACK_NAME.to_string(),
                    source: VIDEO_TRACK_NAME.to_string(),
                },
            )
            .await
            .unwrap();
        relay.received_catalogs().await;

        // Act
        broadcast.remove_track("audio").await.unwrap();
        relay.wait_for_request("PUBLISH_DONE live/cam/audio").await;
        let catalogs = relay.received_catalogs().await;

        // Assert
        let newest: Catalog =
            serde_json::from_slice(&catalogs.last().expect("a catalog must arrive").payload)
                .unwrap();
        assert!(
            newest
                .tracks
                .iter()
                .flatten()
                .all(|track| track.name != "audio"),
            "{newest:?}"
        );
        assert!(
            !relay
                .received_requests()
                .iter()
                .any(|request| request.starts_with("PUBLISH_DONE")),
            "only the audio track ends"
        );
    }

    #[tokio::test]
    async fn a_catalog_update_during_a_catalog_send_is_the_newest_group() {
        // Arrange: the first catalog exceeds the stream window, so its send
        // waits until the relay reads the catalog track
        let (release_catalog, catalog_gate) = oneshot::channel();
        let mut relay = FakeRelay::spawn_gated("catalog-concurrent", None, Some(catalog_gate));
        let broadcast = live_cam(connection_to(&relay));
        let large_video = VideoTrackInfo {
            init_segment: "A".repeat(2 * 1024 * 1024),
            ..video_track_info()
        };
        let update = |track_name: &'static str, info: VideoTrackInfo| {
            let broadcast = broadcast.clone();
            tokio::spawn(async move { broadcast.update_video_catalog(track_name, info).await })
        };
        let first = update("video_1080p", large_video);
        relay
            .wait_for_request(&format!("PUBLISH live/cam/{CATALOG_TRACK_NAME}"))
            .await;

        // Act
        let second = update("video_360p", video_track_info());
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = release_catalog.send(());
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();

        // Assert
        let catalogs = relay.received_catalogs().await;
        let newest = catalogs
            .iter()
            .max_by_key(|object| object.group_id)
            .expect("a catalog must arrive");
        let catalog = String::from_utf8_lossy(&newest.payload);
        assert!(
            catalog.contains("\"video_1080p\"") && catalog.contains("\"video_360p\""),
            "the newest of groups {:?} lacks a track",
            catalogs
                .iter()
                .map(|object| object.group_id)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn sends_keep_going_to_the_old_relay_while_the_new_one_is_slow() {
        // Arrange: relay B holds its PUBLISH_NAMESPACE answer
        let mut relay_a = FakeRelay::spawn("goaway-slow-a");
        let (release_b, gate) = oneshot::channel();
        let mut relay_b = FakeRelay::spawn_gated("goaway-slow-b", Some(gate), None);
        let (_connection, broadcast) = connected_broadcast(&relay_a).await;
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

        // Assert: the send did not wait for relay B, and no track moved yet
        assert!(sent.is_ok(), "a send must not wait for the move");
        assert_eq!(
            relay_b.received_requests(),
            vec!["PUBLISH_NAMESPACE live/cam".to_string()]
        );
        let _ = release_b.send(());
        wait_until_moved(&mut relay_b).await;
    }

    #[tokio::test]
    async fn objects_go_to_both_relays_until_the_old_one_unsubscribes() {
        // Arrange
        let mut relay_a = FakeRelay::spawn("goaway-both-a");
        let mut relay_b = FakeRelay::spawn("goaway-both-b");
        let (_connection, broadcast) = connected_broadcast(&relay_a).await;
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
        wait_until_moved(&mut relay_b).await;

        // Act: two more groups, then relay A unsubscribes
        for group_id in [2, 3] {
            broadcast
                .send_object(VIDEO_TRACK_NAME, keyframe_object(group_id))
                .await
                .unwrap();
        }
        let old_still_open = tokio::time::timeout(Duration::from_millis(200), session_a.closed())
            .await
            .is_err();
        relay_a.unsubscribe_published(&session_a).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        broadcast
            .send_object(VIDEO_TRACK_NAME, keyframe_object(4))
            .await
            .unwrap();

        // Assert
        relay_a.wait_for_video_group(3).await;
        relay_b.wait_for_video_group(3).await;
        assert!(old_still_open, "relay A must stay open while it subscribes");
        let close = tokio::time::timeout(TEST_TIMEOUT, session_a.closed())
            .await
            .expect("relay A must be closed once it unsubscribed");
        assert_eq!(close.code, Some(0x0));
    }
}
