use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Weak},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use moqt::{
    ClientConfig, Endpoint, MigratingSession, RequestErrorCode, Session, SessionEvent,
    TransportStats,
    executor::{self, JoinHandle},
    publish::{Origin, OutgoingObject, Retention, WriteOutcome},
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
    connected: Arc<Mutex<Option<Arc<ConnectedRelay>>>>,
    ledger: StreamLedger,
}

type TrackKey = (String, String);

#[derive(Default)]
struct RelayState {
    announced_namespaces: HashSet<String>,
    catalogs: HashMap<String, CatalogMetadata>,
    catalog_sent: HashMap<String, CatalogSent>,
    /// Per namespace path, the tracks a SUBSCRIBE is accepted for before the
    /// catalog lists them.
    declared_tracks: HashMap<String, HashSet<String>>,
    /// Whether a SUBSCRIBE in the namespace has been accepted. Dropped on
    /// disconnect, which ends every wait for one.
    subscribed: HashMap<String, watch::Sender<bool>>,
    disconnected: bool,
}

struct CatalogSent {
    /// `executor::now()` at the send.
    at: Duration,
    /// The session the catalog was sent on; after a migration the new relay
    /// gets the catalog at the next send.
    session: Weak<Session>,
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

    fn catalog_refresh_due(
        &self,
        namespace_path: &str,
        now: Duration,
        session: &Weak<Session>,
    ) -> bool {
        self.catalog_sent.get(namespace_path).is_some_and(|sent| {
            now.saturating_sub(sent.at) >= CATALOG_REFRESH_INTERVAL
                || !Weak::ptr_eq(&sent.session, session)
        })
    }
}

pub(crate) struct ConnectedRelay {
    session: Arc<MigratingSession>,
    state: Arc<Mutex<RelayState>>,
    catalog_send: Mutex<()>,
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
            connected: Arc::new(Mutex::new(None)),
            ledger: StreamLedger::default(),
        }
    }

    pub fn streams(&self) -> Vec<StreamRecord> {
        self.ledger.snapshot()
    }

    /// Returns once the CONNECTION_CLOSE has been sent; exiting before that
    /// leaves the relay to end the session only after its idle timeout.
    pub async fn close(&self) {
        let connected = self.connected.lock().await.clone();
        if let Some(connected) = connected {
            connected.session.close().await;
        }
    }

    /// `None` until the relay session exists; the lock is also held for the
    /// whole connect, so a caller during it sees `None` as well.
    pub fn transport_stats(&self) -> Option<TransportStats> {
        let connected = self.connected.try_lock().ok()?.clone()?;
        Some(connected.session.session().transport_stats())
    }

    /// The relay session to send on, connected on first use; it follows the
    /// relay's GOAWAY (`moqt::MigratingSession`).
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub(crate) async fn connected(&self) -> Result<Option<Arc<ConnectedRelay>>> {
        let Some(target) = &self.target else {
            return Ok(None);
        };
        let mut connected = self.connected.lock().await;
        if let Some(relay) = connected.as_ref() {
            return Ok(Some(relay.clone()));
        }
        let relay = Arc::new(ConnectedRelay::connect(target, self.ledger.clone()).await?);
        *connected = Some(relay.clone());
        Ok(Some(relay))
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
        let session = Arc::new(
            MigratingSession::connect(endpoint, &target.url)
                .await
                .context("establish moqt session")?,
        );
        let state = Arc::new(Mutex::new(RelayState::default()));
        let event_task = Self::spawn_event_loop(session.clone(), state.clone(), ledger.clone());
        Ok(Self {
            session,
            state,
            catalog_send: Mutex::new(()),
            ledger,
            event_task,
        })
    }

    fn spawn_event_loop(
        session: Arc<MigratingSession>,
        state: Arc<Mutex<RelayState>>,
        ledger: StreamLedger,
    ) -> JoinHandle {
        executor::spawn("broadcast-relay-events", async move {
            let origin = session.origin();
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
                        Self::serve_subscribe(origin, &state, handler).await;
                    }
                    SessionEvent::Unsubscribe(handler) => {
                        let request_id = handler.subscribe_id();
                        match origin.release(&handler).await {
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
                        origin.cancel_fetch(&handler).await;
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
        if self.state.lock().await.catalog_refresh_due(
            &key.0,
            executor::now(),
            &Arc::downgrade(&self.session.session()),
        ) {
            self.send_catalog(&key.0).await?;
        }
        if self.state.lock().await.disconnected {
            bail!("MoQ publisher disconnected");
        }
        let origin = self.session.origin();
        ensure_origin_track(origin, &key).await;
        let bytes = object.payload.len();
        match origin.write(&key.0, &key.1, object).await? {
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
    ///
    /// Sends do not overlap: `Origin::write_group` does not send a group
    /// placed while another write on the track is in flight, and a snapshot
    /// built earlier could otherwise be written last.
    async fn send_catalog(&self, namespace_path: &str) -> Result<()> {
        let _sending = self.catalog_send.lock().await;
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
        self.state.lock().await.catalog_sent.insert(
            namespace_path.to_string(),
            CatalogSent {
                at: executor::now(),
                session: Arc::downgrade(&self.session.session()),
            },
        );
        let bytes = payload.len();
        match self
            .session
            .origin()
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
        ensure_origin_track(self.session.origin(), &key).await;
        self.session.publish(&key.0, &key.1).await
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

    use moqt::{
        DUAL, Subscription, TrackObject, TrackReader,
        object::DataReceiver,
        publish::GroupBoundary,
        server::{CertificateFiles, ServerConfig, write_self_signed_cert},
        subscribe::FilterType,
    };
    use tokio::sync::{mpsc, oneshot};

    use super::*;
    use crate::broadcast::{Broadcast, VIDEO_TRACK_NAME, VideoTrackInfo};

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

        /// PUBLISH_NAMESPACE is answered only once `namespace_gate` fires,
        /// which holds a publisher moving here in the middle of the move. The
        /// catalog track is read only once `catalog_gate` fires.
        fn spawn_gated(
            name: &str,
            namespace_gate: Option<oneshot::Receiver<()>>,
            catalog_gate: Option<oneshot::Receiver<()>>,
        ) -> Self {
            let endpoint =
                Endpoint::<DUAL>::create_server(&self_signed_server_config(name)).unwrap();
            let url = format!("moqt://127.0.0.1:{}", endpoint.local_port().unwrap());
            let (session_sender, session) = oneshot::channel();
            let (request_sender, requests) = mpsc::unbounded_channel();
            let (published_sender, published) = mpsc::unbounded_channel();
            let (video_object_sender, video_objects) = mpsc::unbounded_channel();
            let (catalog_object_sender, catalog_objects) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                let (mut namespace_gate, mut catalog_gate) = (namespace_gate, catalog_gate);
                let handshake = endpoint.accept().await.unwrap().await.unwrap();
                let session = Arc::new(handshake.accept().await.unwrap());
                let _ = session_sender.send(session.clone());
                while let Ok(event) = session.receive_event().await {
                    match event {
                        SessionEvent::PublishNamespace(handler) => {
                            let _ = request_sender
                                .send(format!("PUBLISH_NAMESPACE {}", handler.track_namespace));
                            if let Some(gate) = namespace_gate.take() {
                                let _ = gate.await;
                            }
                            handler.ok().await.unwrap();
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
                                handler.ok(0, FilterType::LargestObject, 0).await.unwrap();
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
                                    let Ok(DataReceiver::Stream(factory)) = session
                                        .subscriber()
                                        .accept_data_receiver(&subscription)
                                        .await
                                    else {
                                        return;
                                    };
                                    let mut reader = TrackReader::new(factory);
                                    while let Ok(Some(object)) = reader.next_object().await {
                                        let _ = sender.send(object);
                                    }
                                });
                            }
                            let _ = published_sender.send(subscription);
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
        let session = Weak::new();
        let mut state = RelayState::default();
        state.catalog_sent.insert(
            "live/test".to_string(),
            CatalogSent {
                at: sent_at,
                session: session.clone(),
            },
        );

        // Act / Assert
        let almost = sent_at + CATALOG_REFRESH_INTERVAL - Duration::from_secs(1);
        assert!(!state.catalog_refresh_due("live/test", almost, &session));
        let due = sent_at + CATALOG_REFRESH_INTERVAL;
        assert!(state.catalog_refresh_due("live/test", due, &session));
        assert!(!state.catalog_refresh_due("live/other", due, &session));
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
    async fn a_catalog_update_during_a_catalog_send_is_the_newest_group() {
        // Arrange: the first catalog exceeds the stream window, so its send
        // waits until the relay reads the catalog track
        let (release_catalog, catalog_gate) = oneshot::channel();
        let mut relay = FakeRelay::spawn_gated("catalog-concurrent", None, Some(catalog_gate));
        let broadcast = Broadcast::new(
            RelayConnection::new(Some(MoqtTarget {
                url: relay.url.clone(),
                auth_token: None,
                verify_certificate: false,
            })),
            vec!["live".to_string(), "cam".to_string()],
        );
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
