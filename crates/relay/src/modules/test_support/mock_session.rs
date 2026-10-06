use std::sync::{Arc, Mutex};

use moqt::{ContentExists, FilterType, GroupOrder, TerminationErrorCode};
use tokio::sync::oneshot;

use crate::modules::{
    auth::verified_token::VerifiedToken,
    domain::{
        pub_sub_directory::entry::PublishDoneReason, session_id::SessionId,
        session_peer::SessionPeer,
    },
    session::{
        Session,
        data_receiver::{fetch_receiver::UpstreamFetchReceiver, receiver::DataReceiver},
        data_sender::{
            DataSender, fetch_sender::FetchSender, stream_sender_factory::StreamSenderFactory,
        },
        handler::{
            fetch::FetchHandler,
            publish::{PublishHandler, SubscribeOption},
            publish_namespace::PublishNamespaceHandler,
            subscribe::SubscribeHandler,
        },
        moqt_session_event::MoqtSessionEvent,
        publisher::{PublishNamespaceResponse, Publisher},
        session_repository::{NewSession, SessionRepository},
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
}

#[derive(Clone, Default)]
pub(crate) struct RecordedControlMessages {
    unsubscribed_request_ids: Arc<Mutex<Vec<u64>>>,
    pub(crate) fetch_cancelled_request_ids: Arc<Mutex<Vec<u64>>>,
    closes: Arc<Mutex<Vec<(TerminationErrorCode, String)>>>,
    publish_namespaces: Arc<Mutex<Vec<String>>>,
    publishes: Arc<Mutex<Vec<SentPublish>>>,
    publish_dones: Arc<Mutex<Vec<(u64, u64)>>>,
}

impl RecordedControlMessages {
    pub(crate) fn unsubscribed_request_ids(&self) -> Vec<u64> {
        self.unsubscribed_request_ids.lock().unwrap().clone()
    }

    pub(crate) fn closes(&self) -> Vec<(TerminationErrorCode, String)> {
        self.closes.lock().unwrap().clone()
    }

    pub(crate) fn publish_namespaces(&self) -> Vec<String> {
        self.publish_namespaces.lock().unwrap().clone()
    }

    pub(crate) fn publishes(&self) -> Vec<SentPublish> {
        self.publishes.lock().unwrap().clone()
    }

    pub(crate) fn publish_dones(&self) -> Vec<(u64, u64)> {
        self.publish_dones.lock().unwrap().clone()
    }
}

#[derive(Clone)]
enum SubscribeAnswer {
    SubscribeOk(Arc<dyn Fn() -> ContentExists + Send + Sync>),
    Never,
}

#[derive(Clone, Copy)]
pub(crate) enum FetchAnswer {
    Never,
    Refuse,
    FetchOk,
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
    fn new(recorded: RecordedControlMessages) -> Self {
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
    Box::new(MockUpstreamSession {
        recorded: RecordedControlMessages::default(),
        answer_subscribe: Some(SubscribeAnswer::SubscribeOk(Arc::new(answer_subscribe))),
        answer_fetch: FetchAnswer::Never,
        transport_stats: moqt::TransportStats::default(),
        answer_publish: None,
    })
}

pub(crate) fn recorded_session_answering_subscribe() -> (Box<dyn Session>, RecordedControlMessages)
{
    let recorded = RecordedControlMessages::default();
    let session = Box::new(MockUpstreamSession {
        recorded: recorded.clone(),
        answer_subscribe: Some(SubscribeAnswer::SubscribeOk(Arc::new(|| {
            ContentExists::False
        }))),
        answer_fetch: FetchAnswer::Never,
        transport_stats: moqt::TransportStats::default(),
        answer_publish: None,
    });
    (session, recorded)
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
    let mut repository = SessionRepository::new();
    let (session_event_sender, _session_event_receiver) = tokio::sync::mpsc::unbounded_channel();
    for (session_id, session) in sessions {
        repository
            .add(
                NewSession {
                    session_id,
                    session,
                    session_span: tracing::Span::none(),
                    peer: SessionPeer::Client,
                    verified_token: verified_token.clone(),
                },
                session_event_sender.clone(),
            )
            .await;
    }
    Arc::new(tokio::sync::Mutex::new(repository))
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
            answer_fetch: self.answer_fetch,
        })
    }

    async fn receive_moqt_session_event(&self) -> anyhow::Result<MoqtSessionEvent> {
        std::future::pending().await
    }

    fn close(&self, code: TerminationErrorCode, reason: &str) {
        self.recorded
            .closes
            .lock()
            .unwrap()
            .push((code, reason.to_string()));
    }

    fn transport_stats(&self) -> moqt::TransportStats {
        self.transport_stats
    }

    fn transport_addresses(&self) -> moqt::TransportAddresses {
        moqt::TransportAddresses::default()
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

    async fn send_publish_namespace_done(&self, _namespace: String) -> anyhow::Result<()> {
        unimplemented!("not used by MockUpstreamSession tests")
    }

    async fn send_publish(
        &self,
        track_namespace: String,
        track_name: String,
        content_exists: ContentExists,
    ) -> anyhow::Result<DownstreamSubscription> {
        self.recorded.publishes.lock().unwrap().push(SentPublish {
            track_namespace: track_namespace.clone(),
            track_name: track_name.clone(),
            content_exists,
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
            }),
        ))
    }

    async fn send_publish_done(
        &self,
        request_id: u64,
        status_code: u64,
        _stream_count: u64,
        _error_reason: String,
    ) -> anyhow::Result<()> {
        self.recorded
            .publish_dones
            .lock()
            .unwrap()
            .push((request_id, status_code));
        Ok(())
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
    ) -> Box<dyn DataSender> {
        unimplemented!("not used by MockUpstreamSession tests")
    }

    async fn new_fetch_sender(&self, _request_id: u64) -> anyhow::Result<Box<dyn FetchSender>> {
        unimplemented!("not used by MockUpstreamSession tests")
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
        _option: SubscribeOption,
    ) -> anyhow::Result<UpstreamSubscription> {
        let answer_subscribe = match &self.answer_subscribe {
            Some(SubscribeAnswer::SubscribeOk(answer_subscribe)) => answer_subscribe,
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
                content_exists: answer_subscribe(),
                filter_type: FilterType::LargestObject,
                delivery_timeout: None,
            }),
        ))
    }

    async fn send_unsubscribe(&self, subscribe_id: u64) -> anyhow::Result<()> {
        self.recorded
            .unsubscribed_request_ids
            .lock()
            .unwrap()
            .push(subscribe_id);
        Ok(())
    }

    async fn send_unsubscribe_namespace(&self, _namespace: String) -> anyhow::Result<()> {
        unimplemented!("not used by MockUpstreamSession tests")
    }

    async fn create_data_receiver(
        &mut self,
        _subscription: &UpstreamSubscription,
    ) -> anyhow::Result<DataReceiver> {
        unimplemented!("not used by MockUpstreamSession tests")
    }

    async fn send_fetch(
        &mut self,
        _track_namespace: String,
        _track_name: String,
        _start_location: moqt::Location,
        end_location: moqt::Location,
        _option: moqt::FetchOption,
    ) -> anyhow::Result<moqt::FetchHandle> {
        match self.answer_fetch {
            FetchAnswer::Never => std::future::pending().await,
            FetchAnswer::Refuse => Err(anyhow::Error::new(moqt::wire::RequestError {
                request_id: 0,
                error_code: 0x4,
                reason_phrase: "no objects".to_string(),
            })),
            FetchAnswer::FetchOk => Ok(moqt::FetchHandle {
                request_id: 0,
                group_order: GroupOrder::Ascending,
                end_of_track: false,
                end_location,
            }),
        }
    }

    async fn create_fetch_receiver(
        &mut self,
        _handle: &moqt::FetchHandle,
    ) -> anyhow::Result<Box<dyn UpstreamFetchReceiver>> {
        Ok(Box::new(PendingFetchReceiver))
    }

    async fn send_fetch_cancel(&self, request_id: u64) -> anyhow::Result<()> {
        self.recorded
            .fetch_cancelled_request_ids
            .lock()
            .unwrap()
            .push(request_id);
        Ok(())
    }
}

struct PendingFetchReceiver;

#[async_trait::async_trait]
impl UpstreamFetchReceiver for PendingFetchReceiver {
    async fn receive(&mut self) -> anyhow::Result<moqt::Fetch> {
        std::future::pending().await
    }
}

#[derive(Clone, Default)]
pub(crate) struct MockSubscribeHandler {
    pub(crate) subscribe_ok_count: Arc<Mutex<usize>>,
    pub(crate) subscribe_errors: Arc<Mutex<Vec<u64>>>,
}

#[async_trait::async_trait]
impl SubscribeHandler for MockSubscribeHandler {
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

    fn _forward(&self) -> bool {
        true
    }

    fn _filter_type(&self) -> FilterType {
        FilterType::LargestObject
    }

    fn _max_cache_duration(&self) -> Option<u64> {
        None
    }

    fn _delivery_timeout(&self) -> Option<u64> {
        None
    }

    fn allocate_track_alias(&self) -> u64 {
        0
    }

    async fn ok_with_track_alias(
        &self,
        _track_alias: u64,
        _expires: u64,
        _content_exists: ContentExists,
    ) -> Result<(), moqt::TransportSendError> {
        *self.subscribe_ok_count.lock().unwrap() += 1;
        Ok(())
    }

    async fn error(
        &self,
        code: u64,
        _reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        self.subscribe_errors.lock().unwrap().push(code);
        Ok(())
    }

    fn to_downstream_subscription(&self, _track_alias: u64) -> DownstreamSubscription {
        make_subscription(FilterType::LargestObject)
    }
}

pub(crate) struct MockFetchHandler;

#[async_trait::async_trait]
impl FetchHandler for MockFetchHandler {
    fn request_id(&self) -> u64 {
        0
    }

    fn group_order(&self) -> GroupOrder {
        GroupOrder::Ascending
    }

    fn fetch_params(&self) -> moqt::wire::FetchParams {
        moqt::wire::FetchParams::Standalone {
            track_namespace: vec!["ns".to_string()],
            track_name: "track".to_string(),
            start_location: moqt::Location {
                group_id: 0,
                object_id: 0,
            },
            end_location: moqt::Location {
                group_id: 1,
                object_id: 0,
            },
        }
    }

    async fn ok(
        &self,
        _end_of_track: bool,
        _end_location: moqt::Location,
    ) -> Result<(), moqt::TransportSendError> {
        Ok(())
    }

    async fn error(&self, _code: u64, _reason: String) -> Result<(), moqt::TransportSendError> {
        Ok(())
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

    fn _delivery_timeout(&self) -> Option<u64> {
        None
    }

    fn _max_cache_duration(&self) -> Option<u64> {
        None
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
        _code: u64,
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
        _code: u64,
        _reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        Ok(())
    }
}
