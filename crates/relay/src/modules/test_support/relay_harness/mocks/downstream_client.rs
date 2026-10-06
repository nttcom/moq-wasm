use tokio::sync::mpsc;

use crate::modules::session::{
    Session,
    data_object::DataObject,
    data_sender::{
        DataSender, fetch_sender::FetchSender, stream_sender_factory::StreamSenderFactory,
    },
    moqt_session_event::MoqtSessionEvent,
    publisher::{PublishNamespaceResponse, Publisher},
    subscriber::Subscriber,
    subscription::DownstreamSubscription,
};

#[derive(Debug)]
pub(crate) enum Sent {
    Object(DataObject),
    Closed,
    Reset(u64),
}

struct MockDataSender {
    sent: mpsc::UnboundedSender<Sent>,
}

#[async_trait::async_trait]
impl DataSender for MockDataSender {
    async fn send_object(&mut self, object: DataObject) -> anyhow::Result<()> {
        self.sent
            .send(Sent::Object(object))
            .map_err(|_| anyhow::anyhow!("subscriber side dropped"))
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        self.sent
            .send(Sent::Closed)
            .map_err(|_| anyhow::anyhow!("subscriber side dropped"))
    }

    async fn reset(&mut self, error_code: u64) -> anyhow::Result<()> {
        self.sent
            .send(Sent::Reset(error_code))
            .map_err(|_| anyhow::anyhow!("subscriber side dropped"))
    }
}

struct MockStreamSenderFactory {
    sent: mpsc::UnboundedSender<Sent>,
    priorities: mpsc::UnboundedSender<moqt::StreamPriority>,
}

#[async_trait::async_trait]
impl StreamSenderFactory for MockStreamSenderFactory {
    async fn next(
        &mut self,
        priority: moqt::StreamPriority,
    ) -> anyhow::Result<Box<dyn DataSender>> {
        self.priorities
            .send(priority)
            .map_err(|_| anyhow::anyhow!("test side dropped"))?;
        Ok(Box::new(MockDataSender {
            sent: self.sent.clone(),
        }))
    }
}

#[derive(Debug)]
pub(crate) struct SentPublishDone {
    pub(crate) request_id: u64,
    pub(crate) status_code: u64,
    pub(crate) stream_count: u64,
}

#[derive(Clone)]
pub(crate) struct MockPublisher {
    sent: mpsc::UnboundedSender<Sent>,
    priorities: mpsc::UnboundedSender<moqt::StreamPriority>,
    publish_done: mpsc::UnboundedSender<SentPublishDone>,
}

pub(crate) struct MockPublisherObservers {
    pub(crate) sent: mpsc::UnboundedReceiver<Sent>,
    pub(crate) priorities: mpsc::UnboundedReceiver<moqt::StreamPriority>,
    pub(crate) publish_done: mpsc::UnboundedReceiver<SentPublishDone>,
}

impl MockPublisher {
    pub(crate) fn channel() -> (Self, MockPublisherObservers) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let (priorities_sender, priorities_receiver) = mpsc::unbounded_channel();
        let (publish_done_sender, publish_done_receiver) = mpsc::unbounded_channel();
        (
            Self {
                sent: sender,
                priorities: priorities_sender,
                publish_done: publish_done_sender,
            },
            MockPublisherObservers {
                sent: receiver,
                priorities: priorities_receiver,
                publish_done: publish_done_receiver,
            },
        )
    }
}

#[async_trait::async_trait]
impl Publisher for MockPublisher {
    async fn send_publish_namespace(
        &self,
        _namespaces: String,
    ) -> anyhow::Result<PublishNamespaceResponse> {
        unreachable!("not used by the egress path under test")
    }

    async fn send_publish_namespace_done(&self, _namespace: String) -> anyhow::Result<()> {
        unreachable!("not used by the egress path under test")
    }

    async fn send_publish(
        &self,
        _track_namespace: String,
        _track_name: String,
        _content_exists: moqt::ContentExists,
    ) -> anyhow::Result<DownstreamSubscription> {
        unreachable!("not used by the egress path under test")
    }

    async fn send_publish_done(
        &self,
        request_id: u64,
        status_code: u64,
        stream_count: u64,
        _error_reason: String,
    ) -> anyhow::Result<()> {
        self.publish_done
            .send(SentPublishDone {
                request_id,
                status_code,
                stream_count,
            })
            .map_err(|_| anyhow::anyhow!("test side dropped"))
    }

    fn new_stream_factory(
        &self,
        _downstream_subscription: &DownstreamSubscription,
    ) -> Box<dyn StreamSenderFactory> {
        Box::new(MockStreamSenderFactory {
            sent: self.sent.clone(),
            priorities: self.priorities.clone(),
        })
    }

    fn new_datagram(
        &self,
        _downstream_subscription: &DownstreamSubscription,
    ) -> Box<dyn DataSender> {
        unreachable!("not used by the egress path under test")
    }

    async fn new_fetch_sender(&self, _request_id: u64) -> anyhow::Result<Box<dyn FetchSender>> {
        unreachable!("not used by the egress path under test")
    }
}

#[derive(Debug)]
pub(crate) enum FetchSent {
    Object(moqt::FetchObjectField),
    Closed,
    Reset(u64),
}

pub(crate) struct MockFetchSender {
    sent: mpsc::UnboundedSender<FetchSent>,
}

impl MockFetchSender {
    pub(crate) fn channel() -> (Self, mpsc::UnboundedReceiver<FetchSent>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sent: sender }, receiver)
    }
}

#[async_trait::async_trait]
impl FetchSender for MockFetchSender {
    async fn send(&self, object: moqt::FetchObjectField) -> anyhow::Result<()> {
        self.sent
            .send(FetchSent::Object(object))
            .map_err(|_| anyhow::anyhow!("subscriber side dropped"))
    }

    async fn close(&self) -> anyhow::Result<()> {
        self.sent
            .send(FetchSent::Closed)
            .map_err(|_| anyhow::anyhow!("subscriber side dropped"))
    }

    async fn reset(&self, error_code: u64) -> anyhow::Result<()> {
        self.sent
            .send(FetchSent::Reset(error_code))
            .map_err(|_| anyhow::anyhow!("subscriber side dropped"))
    }
}

pub(crate) struct MockDownstreamSession {
    pub(crate) publisher: MockPublisher,
}

#[async_trait::async_trait]
impl Session for MockDownstreamSession {
    fn as_publisher(&self) -> Box<dyn Publisher> {
        Box::new(self.publisher.clone())
    }

    fn as_subscriber(&self) -> Box<dyn Subscriber> {
        unreachable!("not used by the egress path under test")
    }

    async fn receive_moqt_session_event(&self) -> anyhow::Result<MoqtSessionEvent> {
        std::future::pending().await
    }

    fn close(&self, _code: moqt::TerminationErrorCode, _reason: &str) {}

    fn transport_stats(&self) -> moqt::TransportStats {
        moqt::TransportStats::default()
    }

    fn transport_addresses(&self) -> moqt::TransportAddresses {
        moqt::TransportAddresses::default()
    }
}
