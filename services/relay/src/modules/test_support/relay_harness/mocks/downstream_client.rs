use moqt::fetch::UnknownRange;
use moqt::object::{DataStreamResetCode, DatagramDelivery};
use moqt::publish::PublishDoneCode;
use tokio::sync::mpsc;

use crate::modules::session::{
    data_object::DataObject,
    data_sender::{
        DataSender, datagram_sender::DatagramSender, fetch_sender::FetchSender,
        stream_sender_factory::StreamSenderFactory,
    },
    publisher::{PublishNamespaceResponse, Publisher},
    subscription::DownstreamSubscription,
};

#[derive(Debug)]
pub(crate) enum Sent {
    Object(DataObject),
    Datagram(moqt::object::Object),
    Reprioritized(moqt::object::StreamPriority),
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

    async fn set_priority(&mut self, priority: moqt::object::StreamPriority) -> anyhow::Result<()> {
        self.sent
            .send(Sent::Reprioritized(priority))
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

/// Takes a datagram whose payload is at most `payload_limit` bytes.
struct MockDatagramSender {
    sent: mpsc::UnboundedSender<Sent>,
    payload_limit: usize,
}

#[async_trait::async_trait]
impl DatagramSender for MockDatagramSender {
    async fn send(&mut self, object: moqt::object::Object) -> anyhow::Result<DatagramDelivery> {
        if object.payload.len() > self.payload_limit {
            return Ok(DatagramDelivery::TooLarge);
        }
        self.sent
            .send(Sent::Datagram(object))
            .map_err(|_| anyhow::anyhow!("subscriber side dropped"))?;
        Ok(DatagramDelivery::Sent)
    }
}

struct MockStreamSenderFactory {
    sent: mpsc::UnboundedSender<Sent>,
    priorities: mpsc::UnboundedSender<moqt::object::StreamPriority>,
    stall_opens: bool,
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
        if self.stall_opens {
            std::future::pending::<()>().await;
        }
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
    stall_opens: bool,
    mixes_forwarding_preferences: bool,
    datagram_payload_limit: usize,
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
                stall_opens: false,
                mixes_forwarding_preferences,
                datagram_payload_limit: usize::MAX,
            },
            MockPublisherObservers {
                sent: receiver,
                priorities: priorities_receiver,
                publish_done: publish_done_receiver,
            },
        )
    }

    /// A subscriber that never grants stream credit: each stream open reports
    /// its priority, then waits forever.
    pub(crate) fn stalling_stream_opens(self) -> Self {
        Self {
            stall_opens: true,
            ..self
        }
    }

    /// `channel`, to a session whose datagrams take at most `limit` bytes of
    /// payload.
    pub(crate) fn channel_taking_datagrams_up_to(limit: usize) -> (Self, MockPublisherObservers) {
        let (publisher, observers) = Self::channel();
        (
            Self {
                datagram_payload_limit: limit,
                ..publisher
            },
            observers,
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
            stall_opens: self.stall_opens,
        })
    }

    fn new_datagram(
        &self,
        _downstream_subscription: &DownstreamSubscription,
    ) -> Box<dyn DatagramSender> {
        Box::new(MockDatagramSender {
            sent: self.sent.clone(),
            payload_limit: self.datagram_payload_limit,
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

/// The draft whose rule `MockFetchSender::report_unknown_range` follows, as
/// `moqt::fetch::FetchDataSender` does.
#[derive(Debug, Clone, Copy)]
pub(crate) enum UnknownRangeDraft {
    /// No End of Unknown Range: every range resets the response.
    Draft14,
    /// A range is marked unless it ends in the group of the previous object
    /// or marker (§11.4.4.2), which resets the response.
    Draft18,
    /// Every range is marked, which no draft does: for tests about the
    /// objects alone.
    AnyRange,
}

/// Records what reaches the wire: a range the draft cannot mark shows up as
/// the reset with UNKNOWN_OBJECT_STATUS.
pub(crate) struct MockFetchSender {
    sent: mpsc::UnboundedSender<FetchSent>,
    draft: UnknownRangeDraft,
    /// The location of the previous object or marker.
    previous: std::sync::Mutex<Option<moqt::object::Location>>,
}

impl MockFetchSender {
    pub(crate) fn new(draft: UnknownRangeDraft) -> (Self, mpsc::UnboundedReceiver<FetchSent>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (
            Self {
                sent: sender,
                draft,
                previous: std::sync::Mutex::default(),
            },
            receiver,
        )
    }

    fn record(&self, sent: FetchSent) -> anyhow::Result<()> {
        self.sent
            .send(sent)
            .map_err(|_| anyhow::anyhow!("subscriber side dropped"))
    }
}

#[async_trait::async_trait]
impl FetchSender for MockFetchSender {
    async fn send(&self, object: moqt::object::Object) -> anyhow::Result<()> {
        *self.previous.lock().unwrap() = Some(object.location);
        self.record(FetchSent::Object(object))
    }

    async fn report_unknown_range(
        &self,
        last: moqt::object::Location,
    ) -> anyhow::Result<UnknownRange> {
        let previous = self.previous.lock().unwrap().replace(last);
        let marks = match self.draft {
            UnknownRangeDraft::Draft14 => false,
            UnknownRangeDraft::Draft18 => {
                previous.is_none_or(|previous| previous.group_id != last.group_id)
            }
            UnknownRangeDraft::AnyRange => true,
        };
        if !marks {
            self.record(FetchSent::Reset(DataStreamResetCode::UnknownObjectStatus))?;
            return Ok(UnknownRange::Reset);
        }
        self.record(FetchSent::UnknownRange(last))?;
        Ok(UnknownRange::Marked)
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
