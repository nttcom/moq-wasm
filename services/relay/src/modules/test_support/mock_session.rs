use moqt::publish::PublishDoneCode;
use moqt::{RequestErrorCode, TrackProperties};
use std::sync::{Arc, Mutex};

use moqt::{
    TerminationErrorCode,
    namespace::{NamespaceInterest, PrefixUpdateAnswer},
    object::{ContentExists, GroupOrder},
    subscribe::FilterType,
};
use tokio::sync::{mpsc, oneshot};

use crate::modules::{
    auth::verified_token::VerifiedToken,
    domain::{
        publish_done_reason::PublishDoneReason, session_id::SessionId, session_peer::SessionPeer,
    },
    session::{
        Session,
        data_receiver::{
            fetch_receiver::UpstreamFetchReceiver,
            receiver::{DataReceiver, UpstreamData},
            stream_receiver::StreamReceiver,
        },
        data_sender::{
            datagram_sender::DatagramSender, fetch_sender::FetchSender,
            stream_sender_factory::StreamSenderFactory,
        },
        handler::{
            fetch::FetchHandler, publish::PublishHandler,
            publish_namespace::PublishNamespaceHandler, subscribe::SubscribeHandler,
            subscribe_namespace::SubscribeNamespaceHandler,
            subscribe_namespace_update::SubscribeNamespaceUpdateHandler,
            subscribe_update::SubscribeUpdateHandler,
        },
        moqt_session_event::MoqtSessionEvent,
        publisher::{PublishNamespaceResponse, Publisher},
        session_event::SessionEvent,
        session_repository::{NewSession, SessionAdded, SessionRepository},
        subscriber::Subscriber,
        subscription::{DownstreamSubscription, UpstreamSubscription},
    },
    test_support::relay_harness::fixtures::subscription::make_subscription,
};

pub(crate) const PUBLISH_REQUEST_ID: u64 = 8;
pub(crate) const PUBLISH_TRACK_ALIAS: u64 = 3;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SentPublish {
    pub(crate) track_namespace: String,
    pub(crate) track_name: String,
    pub(crate) content_exists: ContentExists,
    pub(crate) track_properties: TrackProperties,
    pub(crate) forward: bool,
}

type InjectedSessionEvent = anyhow::Result<MoqtSessionEvent>;

type StreamSource = mpsc::UnboundedReceiver<Box<dyn StreamReceiver>>;

#[derive(Clone)]
pub(crate) struct RecordedControlMessages {
    event_sender: mpsc::UnboundedSender<InjectedSessionEvent>,
    event_receiver: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<InjectedSessionEvent>>>,
    /// Streams the next data receiver yields to the ingest; without them it
    /// pends forever.
    stream_source: Arc<Mutex<Option<StreamSource>>>,
    /// Request ids whose data receiver the ingest created.
    data_receivers: Arc<Mutex<Vec<u64>>>,
    /// Request ids whose ingest dropped its data receiver.
    ingests_stopped: Arc<Mutex<Vec<u64>>>,
    unsubscribed_request_ids: Arc<Mutex<Vec<u64>>>,
    pub(crate) fetch_cancelled_request_ids: Arc<Mutex<Vec<u64>>>,
    fetch_fill_timeouts: Arc<Mutex<Vec<Option<std::time::Duration>>>>,
    /// Request ids of the FETCH responses the relay opened toward this session.
    fetch_senders_opened: Arc<Mutex<Vec<u64>>>,
    subscribed_track_names: Arc<Mutex<Vec<String>>>,
    closes: Arc<Mutex<Vec<(TerminationErrorCode, String)>>>,
    go_aways: Arc<Mutex<Vec<String>>>,
    publish_namespaces: Arc<Mutex<Vec<String>>>,
    publish_namespace_dones: Arc<Mutex<Vec<String>>>,
    namespace_subscriptions: Arc<Mutex<Vec<String>>>,
    publishes: Arc<Mutex<Vec<SentPublish>>>,
    publish_dones: Arc<Mutex<Vec<(u64, PublishDoneCode)>>>,
}

impl Default for RecordedControlMessages {
    fn default() -> Self {
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        Self {
            event_sender,
            event_receiver: Arc::new(tokio::sync::Mutex::new(event_receiver)),
            stream_source: Arc::default(),
            data_receivers: Arc::default(),
            ingests_stopped: Arc::default(),
            unsubscribed_request_ids: Arc::default(),
            fetch_cancelled_request_ids: Arc::default(),
            fetch_fill_timeouts: Arc::default(),
            fetch_senders_opened: Arc::default(),
            subscribed_track_names: Arc::default(),
            closes: Arc::default(),
            go_aways: Arc::default(),
            publish_namespaces: Arc::default(),
            publish_namespace_dones: Arc::default(),
            namespace_subscriptions: Arc::default(),
            publishes: Arc::default(),
            publish_dones: Arc::default(),
        }
    }
}

impl RecordedControlMessages {
    /// Makes the session's next `receive_moqt_session_event` return `event`.
    pub(crate) fn send_event(&self, event: MoqtSessionEvent) {
        self.event_sender
            .send(Ok(event))
            .expect("the mock session owns the event receiver");
    }

    /// Makes the session's `receive_moqt_session_event` fail once.
    pub(crate) fn fail_event_stream(&self) {
        self.event_sender
            .send(Err(anyhow::anyhow!("session event stream failed")))
            .expect("the mock session owns the event receiver");
    }

    pub(crate) fn unsubscribed_request_ids(&self) -> Vec<u64> {
        self.unsubscribed_request_ids.lock().unwrap().clone()
    }

    /// The streams sent here reach the ingest of the next data receiver.
    pub(crate) fn feed_streams(&self) -> mpsc::UnboundedSender<Box<dyn StreamReceiver>> {
        let (stream_sender, stream_receiver) = mpsc::unbounded_channel();
        *self.stream_source.lock().unwrap() = Some(stream_receiver);
        stream_sender
    }

    pub(crate) fn data_receivers(&self) -> Vec<u64> {
        self.data_receivers.lock().unwrap().clone()
    }

    pub(crate) fn ingests_stopped(&self) -> Vec<u64> {
        self.ingests_stopped.lock().unwrap().clone()
    }

    /// Resolves once the ingest of `request_id` created its data receiver.
    pub(crate) async fn wait_ingest_started(&self, request_id: u64) {
        wait_until(|| self.data_receivers().contains(&request_id))
            .await
            .unwrap_or_else(|_| panic!("ingest of request {request_id} never started"));
    }

    /// Resolves once the ingest of `request_id` dropped its data receiver.
    pub(crate) async fn wait_ingest_stopped(&self, request_id: u64) {
        wait_until(|| self.ingests_stopped().contains(&request_id))
            .await
            .unwrap_or_else(|_| panic!("ingest of request {request_id} never stopped"));
    }

    pub(crate) fn fetch_senders_opened(&self) -> Vec<u64> {
        self.fetch_senders_opened.lock().unwrap().clone()
    }

    /// The FILL_TIMEOUT of each FETCH sent on the session.
    pub(crate) fn fetch_fill_timeouts(&self) -> Vec<Option<std::time::Duration>> {
        self.fetch_fill_timeouts.lock().unwrap().clone()
    }

    /// The track name of each SUBSCRIBE sent on the session.
    pub(crate) fn subscribed_track_names(&self) -> Vec<String> {
        self.subscribed_track_names.lock().unwrap().clone()
    }

    pub(crate) fn closes(&self) -> Vec<(TerminationErrorCode, String)> {
        self.closes.lock().unwrap().clone()
    }

    /// The New Session URIs of the GOAWAYs sent on the session.
    pub(crate) fn go_aways(&self) -> Vec<String> {
        self.go_aways.lock().unwrap().clone()
    }

    pub(crate) fn publish_namespaces(&self) -> Vec<String> {
        self.publish_namespaces.lock().unwrap().clone()
    }

    pub(crate) fn publish_namespace_dones(&self) -> Vec<String> {
        self.publish_namespace_dones.lock().unwrap().clone()
    }

    /// `subscribe <prefix>` or `unsubscribe <prefix>` per request, in order.
    pub(crate) fn namespace_subscriptions(&self) -> Vec<String> {
        self.namespace_subscriptions.lock().unwrap().clone()
    }

    pub(crate) fn publishes(&self) -> Vec<SentPublish> {
        self.publishes.lock().unwrap().clone()
    }

    pub(crate) fn publish_dones(&self) -> Vec<(u64, PublishDoneCode)> {
        self.publish_dones.lock().unwrap().clone()
    }
}

#[derive(Clone)]
enum SubscribeAnswer {
    SubscribeOk(Arc<dyn Fn() -> ContentExists + Send + Sync>),
    /// SUBSCRIBE_OK once the gate is notified.
    SubscribeOkAfter(Arc<tokio::sync::Notify>),
    Refuse(RequestErrorCode),
    Never,
}

#[derive(Clone)]
pub(crate) enum FetchAnswer {
    Never,
    Refuse(RequestErrorCode),
    FetchOk,
    /// FETCH_OK, then a draft-14 response stream yielding these, then
    /// nothing.
    Respond(Vec<moqt::fetch::Fetch>),
    /// Like `Respond`, with objects that carry their forwarding preference
    /// (draft-18).
    RespondWithPreference(Vec<moqt::fetch::Fetch>),
    /// FETCH_OK, then a response stream on which moqt finds the track
    /// malformed this way.
    Malformed(moqt::object::MalformedTrackError),
}

type PublishAnswer = Arc<dyn Fn() -> anyhow::Result<bool> + Send + Sync>;

pub(crate) struct MockUpstreamSession {
    recorded: RecordedControlMessages,
    answer_subscribe: Option<SubscribeAnswer>,
    answer_fetch: FetchAnswer,
    transport_stats: moqt::TransportStats,
    answer_publish: Option<PublishAnswer>,
}

impl MockUpstreamSession {
    pub(crate) fn new(recorded: RecordedControlMessages) -> Self {
        Self {
            recorded,
            answer_subscribe: None,
            answer_fetch: FetchAnswer::Never,
            transport_stats: moqt::TransportStats::default(),
            answer_publish: None,
        }
    }
}

pub(crate) fn mock_session_with_transport_stats(
    transport_stats: moqt::TransportStats,
) -> Box<dyn Session> {
    Box::new(MockUpstreamSession {
        transport_stats,
        ..MockUpstreamSession::new(RecordedControlMessages::default())
    })
}

pub(crate) fn mock_session() -> (Arc<dyn Session>, RecordedControlMessages) {
    let recorded = RecordedControlMessages::default();
    let session: Arc<dyn Session> = Arc::new(MockUpstreamSession::new(recorded.clone()));
    (session, recorded)
}

pub(crate) async fn session_repository_with_upstream_session(
    session_id: SessionId,
) -> (
    Arc<tokio::sync::Mutex<SessionRepository>>,
    RecordedControlMessages,
) {
    session_repository_with_upstream_session_token(session_id, VerifiedToken::full_access()).await
}

pub(crate) fn mock_new_session(
    session_id: SessionId,
    verified_token: VerifiedToken,
) -> (NewSession, RecordedControlMessages) {
    let recorded = RecordedControlMessages::default();
    let new_session = NewSession {
        session_id,
        session: Box::new(MockUpstreamSession::new(recorded.clone())),
        session_span: tracing::Span::none(),
        peer: SessionPeer::Client,
        verified_token,
    };
    (new_session, recorded)
}

pub(crate) async fn session_repository_with_upstream_session_token(
    session_id: SessionId,
    verified_token: VerifiedToken,
) -> (
    Arc<tokio::sync::Mutex<SessionRepository>>,
    RecordedControlMessages,
) {
    let recorded = RecordedControlMessages::default();
    let session = Box::new(MockUpstreamSession::new(recorded.clone()));
    let repository = session_repository_with_session(session_id, session, verified_token).await;
    (repository, recorded)
}

pub(crate) fn mock_session_answering_subscribe(
    answer_subscribe: impl Fn() -> ContentExists + Send + Sync + 'static,
) -> Box<dyn Session> {
    recorded_session_answering_subscribe(answer_subscribe).0
}

pub(crate) fn recorded_session_answering_subscribe(
    answer_subscribe: impl Fn() -> ContentExists + Send + Sync + 'static,
) -> (Box<dyn Session>, RecordedControlMessages) {
    let recorded = RecordedControlMessages::default();
    let session = Box::new(MockUpstreamSession {
        recorded: recorded.clone(),
        answer_subscribe: Some(SubscribeAnswer::SubscribeOk(Arc::new(answer_subscribe))),
        answer_fetch: FetchAnswer::Never,
        transport_stats: moqt::TransportStats::default(),
        answer_publish: None,
    });
    (session, recorded)
}

/// Answers SUBSCRIBE_OK once `gate` is notified.
pub(crate) fn recorded_session_answering_subscribe_after(
    gate: Arc<tokio::sync::Notify>,
) -> (Box<dyn Session>, RecordedControlMessages) {
    let recorded = RecordedControlMessages::default();
    let session = Box::new(MockUpstreamSession {
        answer_subscribe: Some(SubscribeAnswer::SubscribeOkAfter(gate)),
        ..MockUpstreamSession::new(recorded.clone())
    });
    (session, recorded)
}

pub(crate) fn mock_session_refusing_subscribe(code: RequestErrorCode) -> Box<dyn Session> {
    Box::new(MockUpstreamSession {
        answer_subscribe: Some(SubscribeAnswer::Refuse(code)),
        ..MockUpstreamSession::new(RecordedControlMessages::default())
    })
}

pub(crate) fn mock_session_never_answering_subscribe() -> Box<dyn Session> {
    Box::new(MockUpstreamSession {
        recorded: RecordedControlMessages::default(),
        answer_subscribe: Some(SubscribeAnswer::Never),
        answer_fetch: FetchAnswer::Never,
        transport_stats: moqt::TransportStats::default(),
        answer_publish: None,
    })
}

pub(crate) fn recorded_session_answering_fetch(
    answer_fetch: FetchAnswer,
) -> (Box<dyn Session>, RecordedControlMessages) {
    let recorded = RecordedControlMessages::default();
    let session = Box::new(MockUpstreamSession {
        answer_fetch,
        ..MockUpstreamSession::new(recorded.clone())
    });
    (session, recorded)
}

pub(crate) fn mock_session_answering_fetch(answer_fetch: FetchAnswer) -> Box<dyn Session> {
    Box::new(MockUpstreamSession {
        recorded: RecordedControlMessages::default(),
        answer_subscribe: None,
        answer_fetch,
        transport_stats: moqt::TransportStats::default(),
        answer_publish: None,
    })
}

/// `answer_publish` runs when PUBLISH arrives; `Ok(forward)` answers PUBLISH_OK
/// with that Forward State and `Err` stands for PUBLISH_ERROR.
pub(crate) fn mock_session_answering_publish(
    answer_publish: impl Fn() -> anyhow::Result<bool> + Send + Sync + 'static,
) -> (Box<dyn Session>, RecordedControlMessages) {
    let recorded = RecordedControlMessages::default();
    let session = Box::new(MockUpstreamSession {
        answer_publish: Some(Arc::new(answer_publish)),
        ..MockUpstreamSession::new(recorded.clone())
    });
    (session, recorded)
}

pub(crate) async fn session_repository_with_session(
    session_id: SessionId,
    session: Box<dyn Session>,
    verified_token: VerifiedToken,
) -> Arc<tokio::sync::Mutex<SessionRepository>> {
    session_repository_with_sessions(vec![(session_id, session)], verified_token).await
}

pub(crate) async fn session_repository_with_sessions(
    sessions: Vec<(SessionId, Box<dyn Session>)>,
    verified_token: VerifiedToken,
) -> Arc<tokio::sync::Mutex<SessionRepository>> {
    let (mut repository, _session_added_receiver) = session_repository();
    for (session_id, session) in sessions {
        repository.add(NewSession {
            session_id,
            session,
            session_span: tracing::Span::none(),
            peer: SessionPeer::Client,
            verified_token: verified_token.clone(),
        });
    }
    Arc::new(tokio::sync::Mutex::new(repository))
}

pub(crate) async fn add_recorded_relay_session(
    repository: &tokio::sync::Mutex<SessionRepository>,
    session_id: SessionId,
) -> RecordedControlMessages {
    let recorded = RecordedControlMessages::default();
    repository.lock().await.add(NewSession {
        session_id,
        session: Box::new(MockUpstreamSession::new(recorded.clone())),
        session_span: tracing::Span::none(),
        peer: SessionPeer::Relay,
        verified_token: VerifiedToken::full_access(),
    });
    recorded
}

pub(crate) fn session_repository_with_session_and_mailbox(
    session_id: SessionId,
    session: Box<dyn Session>,
    verified_token: VerifiedToken,
) -> (
    Arc<tokio::sync::Mutex<SessionRepository>>,
    mpsc::UnboundedReceiver<SessionEvent>,
) {
    let (repository, mut mailbox_receivers) =
        session_repository_with_sessions_and_mailboxes(vec![(session_id, session)], verified_token);
    (repository, mailbox_receivers.remove(0))
}

/// The mailbox receivers come in the order of `sessions`.
pub(crate) fn session_repository_with_sessions_and_mailboxes(
    sessions: Vec<(SessionId, Box<dyn Session>)>,
    verified_token: VerifiedToken,
) -> (
    Arc<tokio::sync::Mutex<SessionRepository>>,
    Vec<mpsc::UnboundedReceiver<SessionEvent>>,
) {
    let (mut repository, mut session_added_receiver) = session_repository();
    let mut mailbox_receivers = Vec::new();
    for (session_id, session) in sessions {
        repository.add(NewSession {
            session_id,
            session,
            session_span: tracing::Span::none(),
            peer: SessionPeer::Client,
            verified_token: verified_token.clone(),
        });
        mailbox_receivers.push(
            session_added_receiver
                .try_recv()
                .expect("add announces the session")
                .mailbox_receiver,
        );
    }
    (
        Arc::new(tokio::sync::Mutex::new(repository)),
        mailbox_receivers,
    )
}

pub(crate) fn session_repository_with_upstream_session_and_mailbox(
    session_id: SessionId,
) -> (
    Arc<tokio::sync::Mutex<SessionRepository>>,
    RecordedControlMessages,
    mpsc::UnboundedReceiver<SessionEvent>,
) {
    let recorded = RecordedControlMessages::default();
    let (repository, mailbox_receiver) = session_repository_with_session_and_mailbox(
        session_id,
        Box::new(MockUpstreamSession::new(recorded.clone())),
        VerifiedToken::full_access(),
    );
    (repository, recorded, mailbox_receiver)
}

/// A repository with no session; every added session is announced on the
/// returned receiver.
pub(crate) fn session_repository() -> (SessionRepository, mpsc::UnboundedReceiver<SessionAdded>) {
    let (session_added_sender, session_added_receiver) = mpsc::unbounded_channel();
    (
        SessionRepository::new(session_added_sender),
        session_added_receiver,
    )
}

/// A repository whose added sessions get no worker.
pub(crate) fn detached_session_repository() -> SessionRepository {
    session_repository().0
}

/// Resolves `Ok` once `condition` holds, `Err` after one second.
pub(crate) async fn wait_until(
    mut condition: impl FnMut() -> bool,
) -> Result<(), tokio::time::error::Elapsed> {
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !condition() {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
}

/// The stream factory of a mock data receiver: yields the fed streams, or
/// pends; records its drop as the end of the ingest.
struct RecordingStreamFactory {
    request_id: u64,
    recorded: RecordedControlMessages,
    source: Option<StreamSource>,
}

#[async_trait::async_trait]
impl DataReceiver for RecordingStreamFactory {
    async fn next(&mut self) -> anyhow::Result<UpstreamData> {
        match &mut self.source {
            Some(source) => source
                .recv()
                .await
                .map(UpstreamData::Subgroup)
                .ok_or_else(|| anyhow::anyhow!("upstream subscription ended")),
            None => std::future::pending().await,
        }
    }

    fn compares_duplicate_status(&self) -> bool {
        true
    }
}

impl Drop for RecordingStreamFactory {
    fn drop(&mut self) {
        self.recorded
            .ingests_stopped
            .lock()
            .unwrap()
            .push(self.request_id);
    }
}

pub(crate) fn runner_stopped(
    runner_stop_receiver: &mut oneshot::Receiver<PublishDoneReason>,
) -> bool {
    matches!(
        runner_stop_receiver.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    )
}

#[async_trait::async_trait]
impl Session for MockUpstreamSession {
    fn as_publisher(&self) -> Box<dyn Publisher> {
        Box::new(MockUpstreamPublisher {
            recorded: self.recorded.clone(),
            answer_publish: self.answer_publish.clone(),
        })
    }

    fn as_subscriber(&self) -> Box<dyn Subscriber> {
        Box::new(MockUpstreamSubscriber {
            recorded: self.recorded.clone(),
            answer_subscribe: self.answer_subscribe.clone(),
            answer_fetch: self.answer_fetch.clone(),
        })
    }

    async fn receive_moqt_session_event(&self) -> anyhow::Result<MoqtSessionEvent> {
        self.recorded
            .event_receiver
            .lock()
            .await
            .recv()
            .await
            .expect("the session's RecordedControlMessages holds the event sender")
    }

    fn close(&self, code: TerminationErrorCode, reason: &str) {
        self.recorded
            .closes
            .lock()
            .unwrap()
            .push((code, reason.to_string()));
    }

    async fn go_away(&self, new_session_uri: String) -> anyhow::Result<()> {
        self.recorded.go_aways.lock().unwrap().push(new_session_uri);
        Ok(())
    }

    fn transport_stats(&self) -> moqt::TransportStats {
        self.transport_stats
    }

    fn transport_addresses(&self) -> moqt::server::TransportAddresses {
        moqt::server::TransportAddresses::default()
    }

    fn url_scheme(&self) -> &'static str {
        "moqt"
    }
}

struct MockUpstreamPublisher {
    recorded: RecordedControlMessages,
    answer_publish: Option<PublishAnswer>,
}

#[async_trait::async_trait]
impl Publisher for MockUpstreamPublisher {
    async fn send_publish_namespace(
        &self,
        namespaces: String,
    ) -> anyhow::Result<PublishNamespaceResponse> {
        self.recorded
            .publish_namespaces
            .lock()
            .unwrap()
            .push(namespaces);
        Ok(Box::pin(std::future::pending()))
    }

    async fn send_publish_namespace_done(&self, namespace: String) -> anyhow::Result<()> {
        self.recorded
            .publish_namespace_dones
            .lock()
            .unwrap()
            .push(namespace);
        Ok(())
    }

    async fn send_publish(
        &self,
        track_namespace: String,
        track_name: String,
        content_exists: ContentExists,
        track_properties: TrackProperties,
        forward: bool,
    ) -> anyhow::Result<DownstreamSubscription> {
        self.recorded.publishes.lock().unwrap().push(SentPublish {
            track_namespace: track_namespace.clone(),
            track_name: track_name.clone(),
            content_exists,
            track_properties,
            forward,
        });
        let Some(answer_publish) = &self.answer_publish else {
            return std::future::pending().await;
        };
        let forward = answer_publish()?;
        Ok(DownstreamSubscription::from(
            moqt::Subscription::PublisherInitiated(moqt::PublisherInitiatedSubscription {
                request_id: PUBLISH_REQUEST_ID,
                track_namespace,
                track_name,
                track_alias: PUBLISH_TRACK_ALIAS,
                group_order: GroupOrder::Ascending,
                content_exists,
                subscriber_priority: 128,
                forward,
                filter_type: FilterType::LargestObject,
                delivery_timeout: None,
                track_properties: TrackProperties::default(),
                last_update: moqt::LastSubscribeUpdate::default(),
            }),
        ))
    }

    async fn send_publish_done(
        &self,
        subscription: &DownstreamSubscription,
        status_code: PublishDoneCode,
        _stream_count: u64,
        _error_reason: String,
    ) -> anyhow::Result<()> {
        self.recorded
            .publish_dones
            .lock()
            .unwrap()
            .push((subscription.request_id(), status_code));
        Ok(())
    }

    fn mixes_forwarding_preferences(&self) -> bool {
        true
    }

    fn new_stream_factory(
        &self,
        _downstream_subscription: &DownstreamSubscription,
    ) -> Box<dyn StreamSenderFactory> {
        unimplemented!("not used by MockUpstreamSession tests")
    }

    fn new_datagram(
        &self,
        _downstream_subscription: &DownstreamSubscription,
    ) -> Box<dyn DatagramSender> {
        unimplemented!("not used by MockUpstreamSession tests")
    }

    async fn new_fetch_sender(&self, request_id: u64) -> anyhow::Result<Box<dyn FetchSender>> {
        self.recorded
            .fetch_senders_opened
            .lock()
            .unwrap()
            .push(request_id);
        let (sender, _sent) = crate::modules::test_support::relay_harness::MockFetchSender::new(
            crate::modules::test_support::relay_harness::UnknownRangeDraft::Draft18,
        );
        Ok(Box::new(sender))
    }
}

struct MockUpstreamSubscriber {
    recorded: RecordedControlMessages,
    answer_subscribe: Option<SubscribeAnswer>,
    answer_fetch: FetchAnswer,
}

#[async_trait::async_trait]
impl Subscriber for MockUpstreamSubscriber {
    async fn send_subscribe(
        &mut self,
        track_namespace: String,
        track_name: String,
        _option: moqt::subscribe::SubscribeOption,
    ) -> anyhow::Result<UpstreamSubscription> {
        self.recorded
            .subscribed_track_names
            .lock()
            .unwrap()
            .push(track_name.clone());
        let content_exists = match &self.answer_subscribe {
            Some(SubscribeAnswer::SubscribeOk(answer_subscribe)) => answer_subscribe(),
            Some(SubscribeAnswer::SubscribeOkAfter(gate)) => {
                gate.notified().await;
                ContentExists::False
            }
            Some(SubscribeAnswer::Refuse(code)) => {
                return Err(anyhow::Error::new(moqt::RequestFailure::Rejected {
                    request_id: 1,
                    code: *code,
                    reason: "refused".to_string(),
                }));
            }
            Some(SubscribeAnswer::Never) => std::future::pending().await,
            None => unimplemented!("not used by MockUpstreamSession tests"),
        };
        Ok(UpstreamSubscription::from(
            moqt::Subscription::SubscriberInitiated(moqt::SubscriberInitiatedSubscription {
                request_id: 1,
                track_namespace,
                track_name,
                track_alias: 0,
                expires: 0,
                group_order: GroupOrder::Ascending,
                subscriber_priority: 128,
                forward: true,
                content_exists,
                filter_type: FilterType::LargestObject,
                track_properties: TrackProperties::default(),
                last_update: moqt::LastSubscribeUpdate::default(),
            }),
        ))
    }

    async fn send_unsubscribe(&self, subscription: &UpstreamSubscription) -> anyhow::Result<()> {
        self.recorded
            .unsubscribed_request_ids
            .lock()
            .unwrap()
            .push(subscription.request_id());
        Ok(())
    }

    async fn send_new_group_request(
        &self,
        _subscription: &UpstreamSubscription,
        _new_group_request: u64,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }

    async fn send_subscribe_namespace(&self, namespace: String) -> anyhow::Result<()> {
        self.recorded
            .namespace_subscriptions
            .lock()
            .unwrap()
            .push(format!("subscribe {namespace}"));
        Ok(())
    }

    async fn send_unsubscribe_namespace(&self, namespace: String) -> anyhow::Result<()> {
        self.recorded
            .namespace_subscriptions
            .lock()
            .unwrap()
            .push(format!("unsubscribe {namespace}"));
        Ok(())
    }

    async fn create_data_receiver(
        &mut self,
        subscription: &UpstreamSubscription,
    ) -> anyhow::Result<Box<dyn DataReceiver>> {
        let request_id = subscription.request_id();
        self.recorded
            .data_receivers
            .lock()
            .unwrap()
            .push(request_id);
        let source = self.recorded.stream_source.lock().unwrap().take();
        Ok(Box::new(RecordingStreamFactory {
            request_id,
            recorded: self.recorded.clone(),
            source,
        }))
    }

    async fn send_fetch(
        &mut self,
        _track_namespace: String,
        _track_name: String,
        _start_location: moqt::object::Location,
        end_location: moqt::object::Location,
        option: moqt::fetch::FetchOption,
    ) -> anyhow::Result<moqt::fetch::FetchHandle> {
        self.recorded
            .fetch_fill_timeouts
            .lock()
            .unwrap()
            .push(option.fill_timeout);
        match self.answer_fetch {
            FetchAnswer::Never => std::future::pending().await,
            FetchAnswer::Refuse(code) => Err(anyhow::Error::new(moqt::RequestFailure::Rejected {
                request_id: 0,
                code,
                reason: "refused".to_string(),
            })),
            FetchAnswer::FetchOk
            | FetchAnswer::Respond(_)
            | FetchAnswer::RespondWithPreference(_)
            | FetchAnswer::Malformed(_) => Ok(moqt::fetch::FetchHandle {
                request_id: 0,
                group_order: GroupOrder::Ascending,
                end_of_track: false,
                end_location,
                track_properties: TrackProperties::default(),
                remainder: None,
            }),
        }
    }

    async fn create_fetch_receiver(
        &mut self,
        _handle: &moqt::fetch::FetchHandle,
    ) -> anyhow::Result<Box<dyn UpstreamFetchReceiver>> {
        match &self.answer_fetch {
            FetchAnswer::Respond(response) => Ok(Box::new(ScriptedFetchReceiver {
                response: response.clone().into(),
                carries_forwarding_preference: false,
            })),
            FetchAnswer::RespondWithPreference(response) => Ok(Box::new(ScriptedFetchReceiver {
                response: response.clone().into(),
                carries_forwarding_preference: true,
            })),
            FetchAnswer::Malformed(error) => Ok(Box::new(MalformedFetchReceiver(error.clone()))),
            _ => Ok(Box::new(PendingFetchReceiver)),
        }
    }

    async fn send_fetch_cancel(&self, fetch: &moqt::fetch::FetchHandle) -> anyhow::Result<()> {
        self.recorded
            .fetch_cancelled_request_ids
            .lock()
            .unwrap()
            .push(fetch.request_id);
        Ok(())
    }
}

struct PendingFetchReceiver;

struct MalformedFetchReceiver(moqt::object::MalformedTrackError);

#[async_trait::async_trait]
impl UpstreamFetchReceiver for MalformedFetchReceiver {
    async fn receive(&mut self) -> anyhow::Result<moqt::fetch::Fetch> {
        Err(self.0.clone().into())
    }

    fn carries_forwarding_preference(&self) -> bool {
        false
    }

    fn compares_duplicate_status(&self) -> bool {
        true
    }
}

struct ScriptedFetchReceiver {
    response: std::collections::VecDeque<moqt::fetch::Fetch>,
    carries_forwarding_preference: bool,
}

#[async_trait::async_trait]
impl UpstreamFetchReceiver for ScriptedFetchReceiver {
    async fn receive(&mut self) -> anyhow::Result<moqt::fetch::Fetch> {
        match self.response.pop_front() {
            Some(fetched) => Ok(fetched),
            None => std::future::pending().await,
        }
    }

    fn carries_forwarding_preference(&self) -> bool {
        self.carries_forwarding_preference
    }

    /// `RespondWithPreference` stands for a draft-18 response.
    fn compares_duplicate_status(&self) -> bool {
        !self.carries_forwarding_preference
    }
}

#[async_trait::async_trait]
impl UpstreamFetchReceiver for PendingFetchReceiver {
    async fn receive(&mut self) -> anyhow::Result<moqt::fetch::Fetch> {
        std::future::pending().await
    }

    fn carries_forwarding_preference(&self) -> bool {
        false
    }

    fn compares_duplicate_status(&self) -> bool {
        true
    }
}

#[derive(Clone)]
pub(crate) struct MockSubscribeHandler {
    pub(crate) forward: bool,
    /// `false` stands in for a subscriber whose draft cannot carry the
    /// track's properties: `ok_with_track_alias` then refuses, as moqt's does.
    pub(crate) understands_track_properties: bool,
    /// The track properties of each SUBSCRIBE_OK sent.
    pub(crate) subscribe_oks: Arc<Mutex<Vec<TrackProperties>>>,
    pub(crate) subscribe_errors: Arc<Mutex<Vec<RequestErrorCode>>>,
    /// Runs when the sequence allocates the track alias, i.e. after it found
    /// the upstream and before it registers the downstream subscription.
    pub(crate) on_allocate_track_alias: Option<Arc<dyn Fn() + Send + Sync>>,
    pub(crate) trace_context: Option<moqt::TraceContext>,
}

impl Default for MockSubscribeHandler {
    fn default() -> Self {
        Self {
            forward: true,
            understands_track_properties: true,
            subscribe_oks: Arc::default(),
            subscribe_errors: Arc::default(),
            on_allocate_track_alias: None,
            trace_context: None,
        }
    }
}

#[async_trait::async_trait]
impl SubscribeHandler for MockSubscribeHandler {
    fn trace_context(&self) -> Option<&moqt::TraceContext> {
        self.trace_context.as_ref()
    }
    fn subscribe_id(&self) -> u64 {
        100
    }

    fn track_namespace(&self) -> &str {
        "ns"
    }

    fn track_namespace_tuple(&self) -> &[String] {
        &[]
    }

    fn track_name(&self) -> &str {
        "track"
    }

    fn _subscriber_priority(&self) -> u8 {
        128
    }

    fn _group_order(&self) -> GroupOrder {
        GroupOrder::Ascending
    }

    fn forward(&self) -> bool {
        self.forward
    }

    fn _filter_type(&self) -> FilterType {
        FilterType::LargestObject
    }

    fn new_group_request(&self) -> Option<u64> {
        None
    }

    fn allocate_track_alias(&self) -> u64 {
        if let Some(hook) = &self.on_allocate_track_alias {
            hook();
        }
        0
    }

    fn understands(&self, _track_properties: &TrackProperties) -> bool {
        self.understands_track_properties
    }

    async fn ok_with_track_alias(
        &self,
        _track_alias: u64,
        _expires: u64,
        _content_exists: ContentExists,
        track_properties: TrackProperties,
    ) -> anyhow::Result<()> {
        if !self.understands_track_properties {
            self.subscribe_errors
                .lock()
                .unwrap()
                .push(RequestErrorCode::UnsupportedExtension);
            anyhow::bail!("the subscriber's draft cannot carry the track's properties");
        }
        self.subscribe_oks.lock().unwrap().push(track_properties);
        Ok(())
    }

    async fn error(
        &self,
        code: RequestErrorCode,
        _reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        self.subscribe_errors.lock().unwrap().push(code);
        Ok(())
    }

    fn to_downstream_subscription(&self, _track_alias: u64) -> DownstreamSubscription {
        make_subscription(FilterType::LargestObject)
    }
}

/// `joining_rule` stands in for the session draft's Joining FETCH rule.
/// Without `understands_track_properties`, `ok` refuses the FETCH as moqt
/// does.
#[derive(Clone)]
pub(crate) struct MockFetchHandler {
    pub(crate) fetch_params: moqt::fetch::FetchParams,
    pub(crate) fill_timeout: Option<std::time::Duration>,
    /// Stands in for the session draft: true for draft-18.
    pub(crate) marks_unknown_ranges: bool,
    pub(crate) understands_track_properties: bool,
    pub(crate) joining_rule:
        fn(moqt::fetch::JoinedSubscription) -> Result<(), moqt::fetch::JoiningFetchRefusal>,
    /// The track properties of each FETCH_OK sent.
    pub(crate) fetch_oks: Arc<Mutex<Vec<TrackProperties>>>,
    pub(crate) errors: Arc<Mutex<Vec<RequestErrorCode>>>,
    pub(crate) oks: Arc<Mutex<Vec<moqt::object::Location>>>,
}

impl MockFetchHandler {
    /// Permits every Joining FETCH.
    pub(crate) fn fetching(fetch_params: moqt::fetch::FetchParams) -> Self {
        Self {
            fetch_params,
            fill_timeout: None,
            marks_unknown_ranges: false,
            understands_track_properties: true,
            joining_rule: |_| Ok(()),
            fetch_oks: Arc::default(),
            errors: Arc::default(),
            oks: Arc::default(),
        }
    }
}

impl Default for MockFetchHandler {
    fn default() -> Self {
        Self::fetching(moqt::fetch::FetchParams::Standalone {
            track_namespace: vec!["ns".to_string()],
            track_name: "track".to_string(),
            start_location: moqt::object::Location {
                group_id: 0,
                object_id: 0,
            },
            end_location: moqt::object::Location {
                group_id: 1,
                object_id: 0,
            },
        })
    }
}

#[async_trait::async_trait]
impl FetchHandler for MockFetchHandler {
    fn trace_context(&self) -> Option<&moqt::TraceContext> {
        None
    }
    fn request_id(&self) -> u64 {
        0
    }

    fn group_order(&self) -> GroupOrder {
        GroupOrder::Ascending
    }

    fn fetch_params(&self) -> moqt::fetch::FetchParams {
        self.fetch_params.clone()
    }

    fn fill_timeout(&self) -> Option<std::time::Duration> {
        self.fill_timeout
    }

    fn marks_unknown_ranges(&self) -> bool {
        self.marks_unknown_ranges
    }

    fn understands(&self, _track_properties: &TrackProperties) -> bool {
        self.understands_track_properties
    }

    fn check_joining(
        &self,
        subscription: moqt::fetch::JoinedSubscription,
    ) -> Result<(), moqt::fetch::JoiningFetchRefusal> {
        (self.joining_rule)(subscription)
    }

    async fn ok(
        &self,
        _end_of_track: bool,
        end_location: moqt::object::Location,
        track_properties: TrackProperties,
    ) -> anyhow::Result<()> {
        self.oks.lock().unwrap().push(end_location);
        if !self.understands_track_properties {
            self.errors
                .lock()
                .unwrap()
                .push(RequestErrorCode::UnsupportedExtension);
            anyhow::bail!("the fetcher's draft cannot carry the track's properties");
        }
        self.fetch_oks.lock().unwrap().push(track_properties);
        Ok(())
    }

    async fn error(
        &self,
        code: RequestErrorCode,
        _reason: String,
    ) -> Result<(), moqt::TransportSendError> {
        self.errors.lock().unwrap().push(code);
        Ok(())
    }
}

/// An update of subscription `subscription_request_id` in a session of
/// `draft`; `answers` records the Largest Object of every `ok`.
pub(crate) struct MockSubscribeUpdateHandler {
    subscription_request_id: u64,
    subscriber_priority: u8,
    forward: bool,
    filter: Option<FilterType>,
    draft: UpdateDraft,
    pub(crate) answers: Mutex<Vec<Option<moqt::object::Location>>>,
}

#[derive(Clone, Copy)]
pub(crate) enum UpdateDraft {
    /// No answer, no widening (draft-14 §9.10).
    Draft14,
    /// REQUEST_OK with LARGEST_OBJECT, widening allowed (draft-18 §10.9.1).
    Draft18,
    /// A draft-18 update already answered REQUEST_ERROR TIMEOUT.
    Draft18TimedOut,
}

impl MockSubscribeUpdateHandler {
    pub(crate) fn new(
        draft: UpdateDraft,
        subscription_request_id: u64,
        forward: bool,
        filter: Option<FilterType>,
    ) -> Self {
        Self {
            subscription_request_id,
            subscriber_priority: 128,
            forward,
            filter,
            draft,
            answers: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn with_subscriber_priority(self, subscriber_priority: u8) -> Self {
        Self {
            subscriber_priority,
            ..self
        }
    }
}

impl SubscribeUpdateHandler for MockSubscribeUpdateHandler {
    fn request_id(&self) -> u64 {
        0
    }

    fn subscription_request_id(&self) -> u64 {
        self.subscription_request_id
    }

    fn subscriber_priority(&self) -> u8 {
        self.subscriber_priority
    }

    fn forward(&self) -> bool {
        self.forward
    }

    fn filter(&self) -> Option<FilterType> {
        self.filter
    }

    fn new_group_request(&self) -> Option<u64> {
        None
    }

    fn may_widen(&self) -> bool {
        !matches!(self.draft, UpdateDraft::Draft14)
    }

    fn ok(&self, largest: Option<moqt::object::Location>) -> moqt::subscribe::UpdateAnswer {
        self.answers.lock().unwrap().push(largest);
        match self.draft {
            UpdateDraft::Draft14 => moqt::subscribe::UpdateAnswer::Accepted {
                largest_told: false,
            },
            UpdateDraft::Draft18 => moqt::subscribe::UpdateAnswer::Accepted { largest_told: true },
            UpdateDraft::Draft18TimedOut => moqt::subscribe::UpdateAnswer::Failed,
        }
    }
}

#[derive(Debug)]
pub(crate) struct MockPublishHandler {
    track_namespace: String,
    track_namespace_tuple: Vec<String>,
    track_name: String,
    track_alias: u64,
}

impl MockPublishHandler {
    pub(crate) fn new(track_namespace: &str, track_name: &str, track_alias: u64) -> Self {
        Self {
            track_namespace: track_namespace.to_string(),
            track_namespace_tuple: track_namespace.split('/').map(str::to_string).collect(),
            track_name: track_name.to_string(),
            track_alias,
        }
    }
}

#[async_trait::async_trait]
impl PublishHandler for MockPublishHandler {
    fn trace_context(&self) -> Option<&moqt::TraceContext> {
        None
    }
    fn track_namespace(&self) -> &str {
        &self.track_namespace
    }

    fn track_namespace_tuple(&self) -> &[String] {
        &self.track_namespace_tuple
    }

    fn track_name(&self) -> &str {
        &self.track_name
    }

    fn track_alias(&self) -> u64 {
        self.track_alias
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
            request_id: 0,
            track_namespace: self.track_namespace.clone(),
            track_name: self.track_name.clone(),
            track_alias: self.track_alias,
            group_order: GroupOrder::Ascending,
            content_exists: ContentExists::False,
            subscriber_priority,
            forward: true,
            filter_type,
            delivery_timeout: None,
            track_properties: TrackProperties::default(),
            last_update: moqt::LastSubscribeUpdate::default(),
        })
    }

    async fn ok(
        &self,
        _subscription: &UpstreamSubscription,
    ) -> Result<(), moqt::TransportSendError> {
        Ok(())
    }

    async fn accept_data_receiver(&self) {}

    async fn error(
        &self,
        _code: RequestErrorCode,
        _reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        Ok(())
    }
}

pub(crate) struct MockPublishNamespaceHandler {
    track_namespace: String,
    track_namespace_tuple: Vec<String>,
}

impl MockPublishNamespaceHandler {
    pub(crate) fn new(track_namespace: &str) -> Self {
        Self {
            track_namespace: track_namespace.to_string(),
            track_namespace_tuple: track_namespace.split('/').map(str::to_string).collect(),
        }
    }
}

#[async_trait::async_trait]
impl PublishNamespaceHandler for MockPublishNamespaceHandler {
    fn trace_context(&self) -> Option<&moqt::TraceContext> {
        None
    }
    fn track_namespace(&self) -> &str {
        &self.track_namespace
    }

    fn track_namespace_tuple(&self) -> &[String] {
        &self.track_namespace_tuple
    }

    async fn ok(&self) -> Result<(), moqt::TransportSendError> {
        Ok(())
    }

    async fn error(
        &self,
        _code: RequestErrorCode,
        _reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        Ok(())
    }
}

/// A draft-18 update moving a namespace subscription; its answers go to
/// `answers`, `None` for REQUEST_OK and the code for REQUEST_ERROR.
#[derive(Clone)]
pub(crate) struct MockSubscribeNamespaceUpdateHandler {
    pub(crate) track_namespace_prefix: String,
    pub(crate) new_track_namespace_prefix: String,
    pub(crate) new_track_namespace_prefix_tuple: Vec<String>,
    pub(crate) interest: NamespaceInterest,
    pub(crate) answers: Arc<Mutex<Vec<Option<RequestErrorCode>>>>,
    /// What `ok` reports.
    pub(crate) ok_answer: PrefixUpdateAnswer,
}

impl MockSubscribeNamespaceUpdateHandler {
    pub(crate) fn new(
        track_namespace_prefix: &str,
        new_track_namespace_prefix: &str,
        interest: NamespaceInterest,
    ) -> Self {
        Self {
            track_namespace_prefix: track_namespace_prefix.to_string(),
            new_track_namespace_prefix: new_track_namespace_prefix.to_string(),
            new_track_namespace_prefix_tuple: new_track_namespace_prefix
                .split('/')
                .filter(|field| !field.is_empty())
                .map(str::to_string)
                .collect(),
            interest,
            answers: Arc::default(),
            ok_answer: PrefixUpdateAnswer::Accepted,
        }
    }

    pub(crate) fn answers(&self) -> Vec<Option<RequestErrorCode>> {
        self.answers.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl SubscribeNamespaceUpdateHandler for MockSubscribeNamespaceUpdateHandler {
    fn track_namespace_prefix(&self) -> &str {
        &self.track_namespace_prefix
    }

    fn new_track_namespace_prefix(&self) -> &str {
        &self.new_track_namespace_prefix
    }

    fn new_track_namespace_prefix_tuple(&self) -> &[String] {
        &self.new_track_namespace_prefix_tuple
    }

    fn interest(&self) -> NamespaceInterest {
        self.interest
    }

    async fn ok(&self) -> PrefixUpdateAnswer {
        self.answers.lock().unwrap().push(None);
        self.ok_answer
    }

    async fn error(&self, code: RequestErrorCode, _reason_phrase: String) {
        self.answers.lock().unwrap().push(Some(code));
    }
}

/// The responses a `MockSubscribeNamespaceHandler` sent, in order.
pub(crate) type SentMessages = Arc<Mutex<Vec<&'static str>>>;

/// A namespace subscription as draft-14 makes it, unless the fields say
/// otherwise; its responses go to `sent`.
#[derive(Clone)]
pub(crate) struct MockSubscribeNamespaceHandler {
    pub(crate) track_namespace_prefix: String,
    pub(crate) track_namespace_prefix_tuple: Vec<String>,
    pub(crate) interest: NamespaceInterest,
    pub(crate) forward: bool,
    pub(crate) sent: SentMessages,
    pub(crate) error_codes: Arc<Mutex<Vec<RequestErrorCode>>>,
}

impl MockSubscribeNamespaceHandler {
    /// The empty prefix stands for the empty tuple.
    pub(crate) fn new(track_namespace_prefix: &str) -> Self {
        Self {
            track_namespace_prefix: track_namespace_prefix.to_string(),
            track_namespace_prefix_tuple: track_namespace_prefix
                .split('/')
                .filter(|field| !field.is_empty())
                .map(str::to_string)
                .collect(),
            interest: NamespaceInterest::NamespacesAndTracks,
            forward: true,
            sent: SentMessages::default(),
            error_codes: Arc::default(),
        }
    }
}

#[async_trait::async_trait]
impl SubscribeNamespaceHandler for MockSubscribeNamespaceHandler {
    fn trace_context(&self) -> Option<&moqt::TraceContext> {
        None
    }

    fn track_namespace_prefix(&self) -> &str {
        &self.track_namespace_prefix
    }

    fn track_namespace_prefix_tuple(&self) -> &[String] {
        &self.track_namespace_prefix_tuple
    }

    fn interest(&self) -> NamespaceInterest {
        self.interest
    }

    fn forward(&self) -> bool {
        self.forward
    }

    async fn ok(&self) -> Result<(), moqt::TransportSendError> {
        self.sent.lock().unwrap().push("SUBSCRIBE_NAMESPACE_OK");
        Ok(())
    }

    async fn error(
        &self,
        code: RequestErrorCode,
        _reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        self.sent.lock().unwrap().push("SUBSCRIBE_NAMESPACE_ERROR");
        self.error_codes.lock().unwrap().push(code);
        Ok(())
    }
}
