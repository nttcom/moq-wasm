use std::sync::{Arc, Mutex};

use crate::modules::{
    core::{
        data_receiver::{fetch_receiver::UpstreamFetchReceiver, receiver::DataReceiver},
        handler::publish::SubscribeOption,
        publisher::Publisher,
        session::Session,
        session_event::MoqtSessionEvent,
        subscriber::Subscriber,
        subscription::UpstreamSubscription,
    },
    session_repository::SessionRepository,
    types::SessionId,
};

/// Control messages recorded by [`MockControlSession`]'s subscriber.
#[derive(Clone, Default)]
pub(crate) struct RecordedControlMessages {
    pub(crate) unsubscribed_request_ids: Arc<Mutex<Vec<u64>>>,
    pub(crate) fetch_cancelled_request_ids: Arc<Mutex<Vec<u64>>>,
}

/// Session mock for control-plane tests: records UNSUBSCRIBE and
/// FETCH_CANCEL, and hands out a fetch receiver that never yields data.
pub(crate) struct MockControlSession {
    recorded: RecordedControlMessages,
}

/// Registers a [`MockControlSession`] in a fresh repository and returns the
/// repository together with the recorders observing the session.
pub(crate) async fn session_repository_with_mock_control_session(
    session_id: SessionId,
) -> (
    Arc<tokio::sync::Mutex<SessionRepository>>,
    RecordedControlMessages,
) {
    let recorded = RecordedControlMessages::default();
    let mut repository = SessionRepository::new();
    let (session_event_sender, _session_event_receiver) = tokio::sync::mpsc::unbounded_channel();
    repository
        .add_client(
            session_id,
            Box::new(MockControlSession {
                recorded: recorded.clone(),
            }),
            session_event_sender,
            tracing::Span::none(),
        )
        .await;
    (Arc::new(tokio::sync::Mutex::new(repository)), recorded)
}

#[async_trait::async_trait]
impl Session for MockControlSession {
    fn as_publisher(&self) -> Box<dyn Publisher> {
        unimplemented!("not used by MockControlSession tests")
    }

    fn as_subscriber(&self) -> Box<dyn Subscriber> {
        Box::new(MockControlSubscriber {
            recorded: self.recorded.clone(),
        })
    }

    async fn receive_moqt_session_event(&self) -> anyhow::Result<MoqtSessionEvent> {
        std::future::pending().await
    }
}

struct MockControlSubscriber {
    recorded: RecordedControlMessages,
}

#[async_trait::async_trait]
impl Subscriber for MockControlSubscriber {
    async fn send_subscribe(
        &mut self,
        _track_namespace: String,
        _track_name: String,
        _option: SubscribeOption,
    ) -> anyhow::Result<UpstreamSubscription> {
        unimplemented!("not used by MockControlSession tests")
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
        unimplemented!("not used by MockControlSession tests")
    }

    async fn create_data_receiver(
        &mut self,
        _subscription: &UpstreamSubscription,
    ) -> anyhow::Result<DataReceiver> {
        unimplemented!("not used by MockControlSession tests")
    }

    async fn send_fetch(
        &mut self,
        _track_namespace: String,
        _track_name: String,
        _start_location: moqt::Location,
        _end_location: moqt::Location,
        _option: moqt::FetchOption,
    ) -> anyhow::Result<moqt::FetchHandle> {
        unimplemented!("not used by MockControlSession tests")
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

/// Never yields data, like an upstream that stalls mid-fetch.
struct PendingFetchReceiver;

#[async_trait::async_trait]
impl UpstreamFetchReceiver for PendingFetchReceiver {
    async fn receive(&mut self) -> anyhow::Result<moqt::Fetch> {
        std::future::pending().await
    }
}
