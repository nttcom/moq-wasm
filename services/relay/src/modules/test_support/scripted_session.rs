//! A session whose peer the test plays. Everything the relay sends the peer
//! is logged in order, and a request of the relay waits until the test
//! answers it, so the test decides the order in which answers reach the relay.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use moqt::{
    RequestErrorCode, TerminationErrorCode, TrackProperties,
    namespace::{NamespaceInterest, PublishNamespaceDoneHandler, UnsubscribeNamespaceHandler},
    object::{ContentExists, GroupOrder},
    publish::PublishDoneCode,
    subscribe::FilterType,
};
use tokio::sync::{mpsc, oneshot};

use crate::modules::{
    auth::verified_token::VerifiedToken,
    domain::{
        publish_done_reason::PublishDoneReason,
        session_id::SessionId,
        session_peer::SessionPeer,
        track_key::{TrackKey, namespace_has_prefix},
    },
    session::{
        Session,
        data_receiver::{
            fetch_receiver::UpstreamFetchReceiver,
            receiver::{DataReceiver, UpstreamData},
        },
        data_sender::{
            datagram_sender::DatagramSender, fetch_sender::FetchSender,
            stream_sender_factory::StreamSenderFactory,
        },
        handler::{
            publish::PublishHandler, publish_namespace::PublishNamespaceHandler,
            subscribe::SubscribeHandler, subscribe_namespace::SubscribeNamespaceHandler,
        },
        moqt_session_event::MoqtSessionEvent,
        publisher::{PublishNamespaceResponse, Publisher},
        session_repository::NewSession,
        subscriber::Subscriber,
        subscription::{DownstreamSubscription, UpstreamSubscription},
    },
};

/// A message the relay sent the peer: an answer to one of the peer's
/// requests, or a request or notification of the relay's own.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Sent {
    PublishNamespaceOk {
        request_id: u64,
        track_namespace: String,
    },
    PublishNamespaceError(String),
    SubscribeNamespaceOk {
        prefix: String,
        interest: NamespaceInterest,
    },
    SubscribeNamespaceError {
        prefix: String,
        interest: NamespaceInterest,
    },
    PublishOk {
        request_id: u64,
    },
    PublishError {
        request_id: u64,
    },
    SubscribeOk {
        request_id: u64,
    },
    SubscribeError {
        request_id: u64,
        code: RequestErrorCode,
    },
    Subscribe {
        request_id: u64,
        track_key: TrackKey,
    },
    Unsubscribe {
        request_id: u64,
    },
    Publish {
        request_id: u64,
        track_key: TrackKey,
    },
    PublishDone {
        request_id: u64,
    },
    PublishNamespace(String),
    PublishNamespaceDone(String),
    Close(TerminationErrorCode),
}

pub(crate) enum SubscribeAnswer {
    Accept,
    Refuse,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum PublishAnswer {
    Accept,
    Refuse,
    /// The peer has no stream left for the PUBLISH (draft-18 §6.1).
    Blocked,
}

struct PeerState {
    sent: Mutex<Vec<Sent>>,
    pending_subscribes: Mutex<BTreeMap<u64, oneshot::Sender<SubscribeAnswer>>>,
    pending_publishes: Mutex<BTreeMap<u64, oneshot::Sender<PublishAnswer>>>,
    /// The relay's requests take the odd Request IDs and the peer's the even
    /// ones, as a server's and a client's do.
    next_relay_request_id: AtomicU64,
    /// The prefixes of the peer's namespace subscriptions, which a draft-18
    /// session holds from sending SUBSCRIBE_NAMESPACE until its
    /// UNSUBSCRIBE_NAMESPACE or the relay's refusal.
    namespace_subscriptions: Mutex<Vec<String>>,
}

impl PeerState {
    fn log(&self, sent: Sent) {
        self.sent.lock().unwrap().push(sent);
    }

    fn relay_request_id(&self) -> u64 {
        self.next_relay_request_id.fetch_add(2, Ordering::Relaxed)
    }

    fn forget_namespace_subscription(&self, prefix: &str) {
        let mut subscriptions = self.namespace_subscriptions.lock().unwrap();
        if let Some(index) = subscriptions.iter().position(|held| held == prefix) {
            subscriptions.remove(index);
        }
    }
}

fn tuple(track_namespace: &str) -> Vec<String> {
    if track_namespace.is_empty() {
        return Vec::new();
    }
    track_namespace.split('/').map(str::to_string).collect()
}

/// The test's hold on a scripted session: it sends the peer's messages,
/// answers the relay's requests and reads what the relay sent.
#[derive(Clone)]
pub(crate) struct ScriptedPeer {
    state: Arc<PeerState>,
    event_sender: mpsc::UnboundedSender<MoqtSessionEvent>,
    next_request_id: Arc<AtomicU64>,
}

impl ScriptedPeer {
    /// A client session to add to the repository, with its peer.
    pub(crate) fn new_session(session_id: SessionId) -> (NewSession, Self) {
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        let state = Arc::new(PeerState {
            sent: Mutex::default(),
            pending_subscribes: Mutex::default(),
            pending_publishes: Mutex::default(),
            next_relay_request_id: AtomicU64::new(1),
            namespace_subscriptions: Mutex::default(),
        });
        let new_session = NewSession {
            session_id,
            session: Box::new(ScriptedSession {
                state: state.clone(),
                event_receiver: tokio::sync::Mutex::new(event_receiver),
            }),
            session_span: tracing::Span::none(),
            peer: SessionPeer::Client,
            verified_token: VerifiedToken::full_access(),
        };
        let peer = Self {
            state,
            event_sender,
            next_request_id: Arc::default(),
        };
        (new_session, peer)
    }

    pub(crate) fn sent(&self) -> Vec<Sent> {
        self.state.sent.lock().unwrap().clone()
    }

    /// The Request IDs of the relay's SUBSCRIBEs the peer has yet to answer.
    pub(crate) fn pending_subscribes(&self) -> Vec<u64> {
        let pending = self.state.pending_subscribes.lock().unwrap();
        pending.keys().copied().collect()
    }

    /// The Request IDs of the relay's PUBLISHes the peer has yet to answer.
    pub(crate) fn pending_publishes(&self) -> Vec<u64> {
        let pending = self.state.pending_publishes.lock().unwrap();
        pending.keys().copied().collect()
    }

    pub(crate) fn answer_subscribe(&self, request_id: u64, answer: SubscribeAnswer) {
        let mut pending = self.state.pending_subscribes.lock().unwrap();
        if let Some(answer_sender) = pending.remove(&request_id) {
            let _ = answer_sender.send(answer);
        }
    }

    pub(crate) fn answer_publish(&self, request_id: u64, answer: PublishAnswer) {
        let mut pending = self.state.pending_publishes.lock().unwrap();
        if let Some(answer_sender) = pending.remove(&request_id) {
            let _ = answer_sender.send(answer);
        }
    }

    fn send(&self, event: MoqtSessionEvent) {
        self.event_sender
            .send(event)
            .expect("the repository holds the session until its worker handled Disconnected");
    }

    fn request_id(&self) -> u64 {
        self.next_request_id.fetch_add(2, Ordering::Relaxed)
    }

    pub(crate) fn publish_namespace(&self, track_namespace: &str) {
        self.send(MoqtSessionEvent::PublishNamespace(Box::new(
            ScriptedPublishNamespace {
                state: self.state.clone(),
                request_id: self.request_id(),
                track_namespace: track_namespace.to_string(),
                track_namespace_tuple: tuple(track_namespace),
            },
        )));
    }

    /// Withdraws the PUBLISH_NAMESPACE `request_id`, as a draft-18 peer does.
    /// `None` withdraws the namespace, as a draft-14 peer does: its
    /// PUBLISH_NAMESPACE_DONE names no request.
    pub(crate) fn publish_namespace_done(&self, track_namespace: &str, request_id: Option<u64>) {
        self.send(MoqtSessionEvent::PublishNamespaceDone(
            PublishNamespaceDoneHandler {
                track_namespace: track_namespace.to_string(),
                track_namespace_tuple: tuple(track_namespace),
                request_id,
            },
        ));
    }

    pub(crate) fn subscribe_namespace(&self, prefix: &str, interest: NamespaceInterest) {
        if interest.includes_namespaces() {
            self.state
                .namespace_subscriptions
                .lock()
                .unwrap()
                .push(prefix.to_string());
        }
        self.send(MoqtSessionEvent::SubscribeNamespace(Box::new(
            ScriptedSubscribeNamespace {
                state: self.state.clone(),
                prefix: prefix.to_string(),
                prefix_tuple: tuple(prefix),
                interest,
            },
        )));
    }

    pub(crate) fn unsubscribe_namespace(&self, prefix: &str, interest: NamespaceInterest) {
        if interest.includes_namespaces() {
            self.state.forget_namespace_subscription(prefix);
        }
        self.send(MoqtSessionEvent::UnsubscribeNamespace(
            UnsubscribeNamespaceHandler {
                track_namespace_prefix: prefix.to_string(),
                interest,
            },
        ));
    }

    /// Returns the Request ID of the PUBLISH.
    pub(crate) fn publish(&self, track_key: &TrackKey) -> u64 {
        let request_id = self.request_id();
        self.send(MoqtSessionEvent::Publish(Box::new(ScriptedPublish {
            state: self.state.clone(),
            request_id,
            track_key: track_key.clone(),
            track_namespace_tuple: tuple(&track_key.track_namespace),
        })));
        request_id
    }

    /// Ends the upstream subscription `request_id`, the peer's PUBLISH or a
    /// SUBSCRIBE of the relay it accepted.
    pub(crate) fn publish_done(&self, request_id: u64) {
        self.send(MoqtSessionEvent::PublishDone {
            request_id,
            reason: PublishDoneReason {
                status_code: PublishDoneCode::TrackEnded,
                error_reason: "track ended".to_string(),
            },
            stream_count: 0,
        });
    }

    /// Returns the Request ID of the SUBSCRIBE.
    pub(crate) fn subscribe(&self, track_key: &TrackKey) -> u64 {
        let request_id = self.request_id();
        self.send(MoqtSessionEvent::Subscribe(Box::new(ScriptedSubscribe {
            state: self.state.clone(),
            request_id,
            track_key: track_key.clone(),
            track_namespace_tuple: tuple(&track_key.track_namespace),
        })));
        request_id
    }

    pub(crate) fn unsubscribe(&self, request_id: u64) {
        self.send(MoqtSessionEvent::Unsubscribe { request_id });
    }

    /// Closes the session: the relay's requests still unanswered fail, as on
    /// a closed connection.
    pub(crate) fn disconnect(&self) {
        self.state.pending_subscribes.lock().unwrap().clear();
        self.state.pending_publishes.lock().unwrap().clear();
        self.send(MoqtSessionEvent::Disconnected());
    }
}

struct ScriptedSession {
    state: Arc<PeerState>,
    event_receiver: tokio::sync::Mutex<mpsc::UnboundedReceiver<MoqtSessionEvent>>,
}

#[async_trait::async_trait]
impl Session for ScriptedSession {
    fn as_publisher(&self) -> Box<dyn Publisher> {
        Box::new(self.state.clone())
    }

    fn as_subscriber(&self) -> Box<dyn Subscriber> {
        Box::new(self.state.clone())
    }

    async fn receive_moqt_session_event(&self) -> anyhow::Result<MoqtSessionEvent> {
        self.event_receiver
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("the test dropped the peer"))
    }

    fn close(&self, code: TerminationErrorCode, _reason: &str) {
        self.state.log(Sent::Close(code));
    }

    async fn go_away(&self, _new_session_uri: String) -> anyhow::Result<()> {
        Ok(())
    }

    fn transport_stats(&self) -> moqt::TransportStats {
        moqt::TransportStats::default()
    }

    fn transport_addresses(&self) -> moqt::server::TransportAddresses {
        moqt::server::TransportAddresses::default()
    }

    fn url_scheme(&self) -> &'static str {
        "moqt"
    }
}

#[async_trait::async_trait]
impl Publisher for Arc<PeerState> {
    async fn send_publish_namespace(
        &self,
        namespaces: String,
    ) -> anyhow::Result<PublishNamespaceResponse> {
        self.log(Sent::PublishNamespace(namespaces));
        Ok(Box::pin(std::future::pending()))
    }

    async fn send_subscribed_namespace(
        &self,
        namespace: String,
    ) -> anyhow::Result<Option<PublishNamespaceResponse>> {
        let covered = self
            .namespace_subscriptions
            .lock()
            .unwrap()
            .iter()
            .any(|prefix| namespace_has_prefix(&namespace, prefix));
        if !covered {
            return Ok(None);
        }
        self.send_publish_namespace(namespace).await.map(Some)
    }

    async fn send_publish_namespace_done(&self, namespace: String) -> anyhow::Result<()> {
        self.log(Sent::PublishNamespaceDone(namespace));
        Ok(())
    }

    async fn send_publish(
        &self,
        track_namespace: String,
        track_name: String,
        content_exists: ContentExists,
        _track_properties: TrackProperties,
        forward: bool,
    ) -> anyhow::Result<DownstreamSubscription> {
        let request_id = self.relay_request_id();
        let (answer_sender, answer_receiver) = oneshot::channel();
        self.log(Sent::Publish {
            request_id,
            track_key: TrackKey::new(&track_namespace, &track_name),
        });
        self.pending_publishes
            .lock()
            .unwrap()
            .insert(request_id, answer_sender);
        match answer_receiver.await {
            Ok(PublishAnswer::Accept) => Ok(DownstreamSubscription::from(
                moqt::Subscription::PublisherInitiated(moqt::PublisherInitiatedSubscription {
                    request_id,
                    track_namespace,
                    track_name,
                    track_alias: request_id,
                    group_order: GroupOrder::Ascending,
                    content_exists,
                    subscriber_priority: 128,
                    forward,
                    filter_type: FilterType::LargestObject,
                    delivery_timeout: None,
                    new_group_request: None,
                    track_properties: TrackProperties::default(),
                    last_update: moqt::LastSubscribeUpdate::default(),
                }),
            )),
            Ok(PublishAnswer::Refuse) => anyhow::bail!("PUBLISH_ERROR"),
            Ok(PublishAnswer::Blocked) => {
                Err(anyhow::Error::new(moqt::RequestFailure::PublishBlocked))
            }
            Err(_) => anyhow::bail!("session closed"),
        }
    }

    async fn send_publish_done(
        &self,
        subscription: &DownstreamSubscription,
        _status_code: PublishDoneCode,
        _stream_count: u64,
        _error_reason: String,
    ) -> anyhow::Result<()> {
        self.log(Sent::PublishDone {
            request_id: subscription.request_id(),
        });
        Ok(())
    }

    fn mixes_forwarding_preferences(&self) -> bool {
        true
    }

    fn new_stream_factory(
        &self,
        _downstream_subscription: &DownstreamSubscription,
    ) -> Box<dyn StreamSenderFactory> {
        unimplemented!("a scripted peer publishes no object")
    }

    fn new_datagram(
        &self,
        _downstream_subscription: &DownstreamSubscription,
    ) -> Box<dyn DatagramSender> {
        unimplemented!("a scripted peer publishes no object")
    }

    async fn new_fetch_sender(&self, _request_id: u64) -> anyhow::Result<Box<dyn FetchSender>> {
        unimplemented!("a scripted peer sends no FETCH")
    }
}

#[async_trait::async_trait]
impl Subscriber for Arc<PeerState> {
    async fn send_subscribe(
        &mut self,
        track_namespace: String,
        track_name: String,
        _option: moqt::subscribe::SubscribeOption,
    ) -> anyhow::Result<UpstreamSubscription> {
        let request_id = self.relay_request_id();
        let (answer_sender, answer_receiver) = oneshot::channel();
        self.log(Sent::Subscribe {
            request_id,
            track_key: TrackKey::new(&track_namespace, &track_name),
        });
        self.pending_subscribes
            .lock()
            .unwrap()
            .insert(request_id, answer_sender);
        match answer_receiver.await {
            Ok(SubscribeAnswer::Accept) => Ok(UpstreamSubscription::from(
                moqt::Subscription::SubscriberInitiated(moqt::SubscriberInitiatedSubscription {
                    request_id,
                    track_namespace,
                    track_name,
                    track_alias: request_id,
                    expires: 0,
                    group_order: GroupOrder::Ascending,
                    subscriber_priority: 128,
                    forward: true,
                    content_exists: ContentExists::False,
                    filter_type: FilterType::LargestObject,
                    track_properties: TrackProperties::default(),
                    last_update: moqt::LastSubscribeUpdate::default(),
                }),
            )),
            Ok(SubscribeAnswer::Refuse) => {
                Err(anyhow::Error::new(moqt::RequestFailure::Rejected {
                    request_id,
                    code: RequestErrorCode::TrackDoesNotExist,
                    reason: "refused".to_string(),
                }))
            }
            Err(_) => anyhow::bail!("session closed"),
        }
    }

    async fn send_unsubscribe(&self, subscription: &UpstreamSubscription) -> anyhow::Result<()> {
        self.log(Sent::Unsubscribe {
            request_id: subscription.request_id(),
        });
        Ok(())
    }

    async fn send_new_group_request(
        &self,
        _subscription: &UpstreamSubscription,
        _new_group_request: u64,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }

    async fn send_forward_update(
        &self,
        _subscription: &UpstreamSubscription,
        _forward: bool,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn send_subscribe_namespace(&self, _namespace: String) -> anyhow::Result<()> {
        unimplemented!("a scripted peer is a client; the relay subscribes namespaces at relays")
    }

    async fn send_unsubscribe_namespace(&self, _namespace: String) -> anyhow::Result<()> {
        unimplemented!("a scripted peer is a client; the relay subscribes namespaces at relays")
    }

    async fn create_data_receiver(
        &mut self,
        _subscription: &UpstreamSubscription,
    ) -> anyhow::Result<Box<dyn DataReceiver>> {
        Ok(Box::new(NoData))
    }

    async fn send_fetch(
        &mut self,
        _track_namespace: String,
        _track_name: String,
        _start_location: moqt::object::Location,
        _end_location: moqt::object::Location,
        _option: moqt::fetch::FetchOption,
    ) -> anyhow::Result<moqt::fetch::FetchHandle> {
        unimplemented!("a scripted peer sends no FETCH")
    }

    async fn create_fetch_receiver(
        &mut self,
        _handle: &moqt::fetch::FetchHandle,
    ) -> anyhow::Result<Box<dyn UpstreamFetchReceiver>> {
        unimplemented!("a scripted peer sends no FETCH")
    }

    async fn send_fetch_cancel(&self, _fetch: &moqt::fetch::FetchHandle) -> anyhow::Result<()> {
        unimplemented!("a scripted peer sends no FETCH")
    }
}

struct NoData;

#[async_trait::async_trait]
impl DataReceiver for NoData {
    async fn next(&mut self) -> anyhow::Result<UpstreamData> {
        std::future::pending().await
    }

    fn compares_duplicate_status(&self) -> bool {
        true
    }
}

struct ScriptedPublishNamespace {
    state: Arc<PeerState>,
    request_id: u64,
    track_namespace: String,
    track_namespace_tuple: Vec<String>,
}

#[async_trait::async_trait]
impl PublishNamespaceHandler for ScriptedPublishNamespace {
    fn trace_context(&self) -> Option<&moqt::TraceContext> {
        None
    }

    fn request_id(&self) -> u64 {
        self.request_id
    }

    fn track_namespace(&self) -> &str {
        &self.track_namespace
    }

    fn track_namespace_tuple(&self) -> &[String] {
        &self.track_namespace_tuple
    }

    async fn ok(&self) -> Result<(), moqt::TransportSendError> {
        self.state.log(Sent::PublishNamespaceOk {
            request_id: self.request_id,
            track_namespace: self.track_namespace.clone(),
        });
        Ok(())
    }

    async fn error(
        &self,
        _code: RequestErrorCode,
        _reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        self.state
            .log(Sent::PublishNamespaceError(self.track_namespace.clone()));
        Ok(())
    }
}

struct ScriptedSubscribeNamespace {
    state: Arc<PeerState>,
    prefix: String,
    prefix_tuple: Vec<String>,
    interest: NamespaceInterest,
}

#[async_trait::async_trait]
impl SubscribeNamespaceHandler for ScriptedSubscribeNamespace {
    fn trace_context(&self) -> Option<&moqt::TraceContext> {
        None
    }

    fn track_namespace_prefix(&self) -> &str {
        &self.prefix
    }

    fn track_namespace_prefix_tuple(&self) -> &[String] {
        &self.prefix_tuple
    }

    fn interest(&self) -> NamespaceInterest {
        self.interest
    }

    fn forward(&self) -> bool {
        true
    }

    async fn ok(&self) -> Result<(), moqt::TransportSendError> {
        self.state.log(Sent::SubscribeNamespaceOk {
            prefix: self.prefix.clone(),
            interest: self.interest,
        });
        Ok(())
    }

    async fn error(
        &self,
        _code: RequestErrorCode,
        _reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        if self.interest.includes_namespaces() {
            self.state.forget_namespace_subscription(&self.prefix);
        }
        self.state.log(Sent::SubscribeNamespaceError {
            prefix: self.prefix.clone(),
            interest: self.interest,
        });
        Ok(())
    }
}

struct ScriptedPublish {
    state: Arc<PeerState>,
    request_id: u64,
    track_key: TrackKey,
    track_namespace_tuple: Vec<String>,
}

impl std::fmt::Debug for ScriptedPublish {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedPublish")
            .field("request_id", &self.request_id)
            .field("track_key", &self.track_key)
            .finish()
    }
}

#[async_trait::async_trait]
impl PublishHandler for ScriptedPublish {
    fn trace_context(&self) -> Option<&moqt::TraceContext> {
        None
    }

    fn track_namespace(&self) -> &str {
        &self.track_key.track_namespace
    }

    fn track_namespace_tuple(&self) -> &[String] {
        &self.track_namespace_tuple
    }

    fn track_name(&self) -> &str {
        &self.track_key.track_name
    }

    fn track_alias(&self) -> u64 {
        self.request_id
    }

    fn _group_order(&self) -> GroupOrder {
        GroupOrder::Ascending
    }

    fn _content_exists(&self) -> ContentExists {
        ContentExists::False
    }

    fn _forward(&self) -> bool {
        true
    }

    fn subscription(
        &self,
        subscriber_priority: u8,
        filter_type: FilterType,
    ) -> UpstreamSubscription {
        UpstreamSubscription::from(moqt::PublisherInitiatedSubscription {
            request_id: self.request_id,
            track_namespace: self.track_key.track_namespace.clone(),
            track_name: self.track_key.track_name.clone(),
            track_alias: self.request_id,
            group_order: GroupOrder::Ascending,
            content_exists: ContentExists::False,
            subscriber_priority,
            forward: true,
            filter_type,
            delivery_timeout: None,
            new_group_request: None,
            track_properties: TrackProperties::default(),
            last_update: moqt::LastSubscribeUpdate::default(),
        })
    }

    async fn accept_data_receiver(&self) {}

    async fn ok(
        &self,
        _subscription: &UpstreamSubscription,
        _forward: bool,
    ) -> Result<(), moqt::TransportSendError> {
        self.state.log(Sent::PublishOk {
            request_id: self.request_id,
        });
        Ok(())
    }

    async fn error(
        &self,
        _code: RequestErrorCode,
        _reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        self.state.log(Sent::PublishError {
            request_id: self.request_id,
        });
        Ok(())
    }
}

struct ScriptedSubscribe {
    state: Arc<PeerState>,
    request_id: u64,
    track_key: TrackKey,
    track_namespace_tuple: Vec<String>,
}

#[async_trait::async_trait]
impl SubscribeHandler for ScriptedSubscribe {
    fn trace_context(&self) -> Option<&moqt::TraceContext> {
        None
    }

    fn subscribe_id(&self) -> u64 {
        self.request_id
    }

    fn track_namespace(&self) -> &str {
        &self.track_key.track_namespace
    }

    fn track_namespace_tuple(&self) -> &[String] {
        &self.track_namespace_tuple
    }

    fn track_name(&self) -> &str {
        &self.track_key.track_name
    }

    fn _subscriber_priority(&self) -> u8 {
        128
    }

    fn _group_order(&self) -> GroupOrder {
        GroupOrder::Ascending
    }

    fn forward(&self) -> bool {
        true
    }

    fn _filter_type(&self) -> FilterType {
        FilterType::LargestObject
    }

    fn new_group_request(&self) -> Option<u64> {
        None
    }

    fn delivery_timeout(&self) -> Option<u64> {
        None
    }

    fn allocate_track_alias(&self) -> u64 {
        self.request_id
    }

    fn understands(&self, _track_properties: &TrackProperties) -> bool {
        true
    }

    async fn ok_with_track_alias(
        &self,
        _track_alias: u64,
        _expires: u64,
        _content_exists: ContentExists,
        _track_properties: TrackProperties,
    ) -> anyhow::Result<()> {
        self.state.log(Sent::SubscribeOk {
            request_id: self.request_id,
        });
        Ok(())
    }

    async fn error(
        &self,
        code: RequestErrorCode,
        _reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        self.state.log(Sent::SubscribeError {
            request_id: self.request_id,
            code,
        });
        Ok(())
    }

    fn to_downstream_subscription(&self, track_alias: u64) -> DownstreamSubscription {
        DownstreamSubscription::from(moqt::Subscription::SubscriberInitiated(
            moqt::SubscriberInitiatedSubscription {
                request_id: self.request_id,
                track_namespace: self.track_key.track_namespace.clone(),
                track_name: self.track_key.track_name.clone(),
                track_alias,
                expires: 0,
                group_order: GroupOrder::Ascending,
                subscriber_priority: 128,
                forward: true,
                content_exists: ContentExists::False,
                filter_type: FilterType::LargestObject,
                track_properties: TrackProperties::default(),
                last_update: moqt::LastSubscribeUpdate::default(),
            },
        ))
    }
}
