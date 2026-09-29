use std::sync::{Arc, Mutex};

use moqt::{ContentExists, FilterType, GroupOrder, TerminationErrorCode};
use tokio::sync::oneshot;

use crate::modules::{
    auth::verified_token::VerifiedToken,
    core::{
        data_receiver::{fetch_receiver::UpstreamFetchReceiver, receiver::DataReceiver},
        handler::{fetch::FetchHandler, publish::SubscribeOption, subscribe::SubscribeHandler},
        publisher::Publisher,
        session::Session,
        session_event::MoqtSessionEvent,
        subscriber::Subscriber,
        subscription::{DownstreamSubscription, UpstreamSubscription},
    },
    relay::tests::harness::fixtures::subscription::make_subscription,
    session_repository::{NewSession, SessionPeer, SessionRepository},
    types::SessionId,
};

#[derive(Clone, Default)]
pub(crate) struct RecordedControlMessages {
    unsubscribed_request_ids: Arc<Mutex<Vec<u64>>>,
    pub(crate) fetch_cancelled_request_ids: Arc<Mutex<Vec<u64>>>,
    closes: Arc<Mutex<Vec<(TerminationErrorCode, String)>>>,
}

impl RecordedControlMessages {
    pub(crate) fn unsubscribed_request_ids(&self) -> Vec<u64> {
        self.unsubscribed_request_ids.lock().unwrap().clone()
    }

    pub(crate) fn closes(&self) -> Vec<(TerminationErrorCode, String)> {
        self.closes.lock().unwrap().clone()
    }
}

type SubscribeAnswer = Arc<dyn Fn() -> ContentExists + Send + Sync>;

pub(crate) struct MockUpstreamSession {
    recorded: RecordedControlMessages,
    answer_subscribe: Option<SubscribeAnswer>,
}

impl MockUpstreamSession {
    fn new(recorded: RecordedControlMessages) -> Self {
        Self {
            recorded,
            answer_subscribe: None,
        }
    }
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
        answer_subscribe: Some(Arc::new(answer_subscribe)),
    })
}

pub(crate) async fn session_repository_with_session(
    session_id: SessionId,
    session: Box<dyn Session>,
    verified_token: VerifiedToken,
) -> Arc<tokio::sync::Mutex<SessionRepository>> {
    let mut repository = SessionRepository::new();
    let (session_event_sender, _session_event_receiver) = tokio::sync::mpsc::unbounded_channel();
    repository
        .add(
            NewSession {
                session_id,
                session,
                session_span: tracing::Span::none(),
                peer: SessionPeer::Client,
                verified_token,
            },
            session_event_sender,
        )
        .await;
    Arc::new(tokio::sync::Mutex::new(repository))
}

pub(crate) fn runner_stopped(runner_stop_receiver: &mut oneshot::Receiver<()>) -> bool {
    matches!(
        runner_stop_receiver.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    )
}

#[async_trait::async_trait]
impl Session for MockUpstreamSession {
    fn as_publisher(&self) -> Box<dyn Publisher> {
        unimplemented!("not used by MockUpstreamSession tests")
    }

    fn as_subscriber(&self) -> Box<dyn Subscriber> {
        Box::new(MockUpstreamSubscriber {
            recorded: self.recorded.clone(),
            answer_subscribe: self.answer_subscribe.clone(),
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
}

struct MockUpstreamSubscriber {
    recorded: RecordedControlMessages,
    answer_subscribe: Option<SubscribeAnswer>,
}

#[async_trait::async_trait]
impl Subscriber for MockUpstreamSubscriber {
    async fn send_subscribe(
        &mut self,
        track_namespace: String,
        track_name: String,
        _option: SubscribeOption,
    ) -> anyhow::Result<UpstreamSubscription> {
        let Some(answer_subscribe) = &self.answer_subscribe else {
            unimplemented!("not used by MockUpstreamSession tests")
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
        _end_location: moqt::Location,
        _option: moqt::FetchOption,
    ) -> anyhow::Result<moqt::FetchHandle> {
        std::future::pending().await
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

#[derive(Default)]
pub(crate) struct MockSubscribeHandler {
    pub(crate) subscribe_ok_count: Mutex<usize>,
    pub(crate) subscribe_errors: Mutex<Vec<moqt::RequestErrorCode>>,
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
        code: moqt::RequestErrorCode,
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

    async fn error(
        &self,
        _code: moqt::RequestErrorCode,
        _reason: String,
    ) -> Result<(), moqt::TransportSendError> {
        Ok(())
    }
}
