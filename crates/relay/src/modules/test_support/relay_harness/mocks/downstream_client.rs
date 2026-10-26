use moqt::fetch::UnknownRange;
use moqt::object::DataStreamResetCode;
use moqt::publish::PublishDoneCode;
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
    Reset(DataStreamResetCode),
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

    async fn reset(&mut self, error_code: DataStreamResetCode) -> anyhow::Result<()> {
        self.sent
            .send(Sent::Reset(error_code))
            .map_err(|_| anyhow::anyhow!("subscriber side dropped"))
    }
}

struct MockStreamSenderFactory {
    sent: mpsc::UnboundedSender<Sent>,
    priorities: mpsc::UnboundedSender<moqt::object::StreamPriority>,
}

#[async_trait::async_trait]
impl StreamSenderFactory for MockStreamSenderFactory {
    async fn next(
        &mut self,
        priority: moqt::object::StreamPriority,
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
    pub(crate) status_code: PublishDoneCode,
    pub(crate) stream_count: u64,
}

#[derive(Clone)]
pub(crate) struct MockPublisher {
    sent: mpsc::UnboundedSender<Sent>,
    priorities: mpsc::UnboundedSender<moqt::object::StreamPriority>,
    publish_done: mpsc::UnboundedSender<SentPublishDone>,
    mixes_forwarding_preferences: bool,
}

pub(crate) struct MockPublisherObservers {
    pub(crate) sent: mpsc::UnboundedReceiver<Sent>,
    pub(crate) priorities: mpsc::UnboundedReceiver<moqt::object::StreamPriority>,
    pub(crate) publish_done: mpsc::UnboundedReceiver<SentPublishDone>,
}

impl MockPublisher {
    pub(crate) fn channel() -> (Self, MockPublisherObservers) {
        Self::channel_mixing(true)
    }

    pub(crate) fn channel_mixing(
        mixes_forwarding_preferences: bool,
    ) -> (Self, MockPublisherObservers) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let (priorities_sender, priorities_receiver) = mpsc::unbounded_channel();
        let (publish_done_sender, publish_done_receiver) = mpsc::unbounded_channel();
        (
            Self {
                sent: sender,
                priorities: priorities_sender,
                publish_done: publish_done_sender,
                mixes_forwarding_preferences,
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
        _content_exists: moqt::object::ContentExists,
        _track_properties: moqt::TrackProperties,
        _forward: bool,
    ) -> anyhow::Result<DownstreamSubscription> {
        unreachable!("not used by the egress path under test")
    }

    async fn send_publish_done(
        &self,
        subscription: &DownstreamSubscription,
        status_code: PublishDoneCode,
        stream_count: u64,
        _error_reason: String,
    ) -> anyhow::Result<()> {
        self.publish_done
            .send(SentPublishDone {
                request_id: subscription.request_id(),
                status_code,
                stream_count,
            })
            .map_err(|_| anyhow::anyhow!("test side dropped"))
    }

    fn mixes_forwarding_preferences(&self) -> bool {
        self.mixes_forwarding_preferences
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
        Box::new(MockDataSender {
            sent: self.sent.clone(),
        })
    }

    async fn new_fetch_sender(&self, _request_id: u64) -> anyhow::Result<Box<dyn FetchSender>> {
        unreachable!("not used by the egress path under test")
    }
}

#[derive(Debug)]
pub(crate) enum FetchSent {
    Object(moqt::object::Object),
    UnknownRange(moqt::object::Location),
    Closed,
    Reset(DataStreamResetCode),
}

/// `unknown_range` is what every `report_unknown_range` returns: `Marked`
/// as on draft-18, `Reset` as on draft-14.
pub(crate) struct MockFetchSender {
    sent: mpsc::UnboundedSender<FetchSent>,
    unknown_range: UnknownRange,
}

impl MockFetchSender {
    pub(crate) fn answering_unknown_range(
        unknown_range: UnknownRange,
    ) -> (Self, mpsc::UnboundedReceiver<FetchSent>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (
            Self {
                sent: sender,
                unknown_range,
            },
            receiver,
        )
    }
}

#[async_trait::async_trait]
impl FetchSender for MockFetchSender {
    async fn send(&self, object: moqt::object::Object) -> anyhow::Result<()> {
        self.sent
            .send(FetchSent::Object(object))
            .map_err(|_| anyhow::anyhow!("subscriber side dropped"))
    }

    async fn report_unknown_range(
        &self,
        last: moqt::object::Location,
    ) -> anyhow::Result<UnknownRange> {
        self.sent
            .send(FetchSent::UnknownRange(last))
            .map_err(|_| anyhow::anyhow!("subscriber side dropped"))?;
        Ok(self.unknown_range)
    }

    async fn close(&self) -> anyhow::Result<()> {
        self.sent
            .send(FetchSent::Closed)
            .map_err(|_| anyhow::anyhow!("subscriber side dropped"))
    }

    async fn reset(&self, error_code: DataStreamResetCode) -> anyhow::Result<()> {
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
