use moqt::fetch::UnknownRange;
use moqt::object::{DataStreamResetCode, DatagramDelivery, SubgroupPart, UnencodableObject};
use moqt::publish::PublishDoneCode;
use tokio::sync::mpsc;

use crate::modules::session::{
    data_sender::{
        DataSender, datagram_sender::DatagramSender, fetch_sender::FetchSender,
        stream_sender_factory::StreamSenderFactory,
    },
    publisher::{PublishNamespaceResponse, Publisher},
    subscription::DownstreamSubscription,
};

#[derive(Debug)]
pub(crate) enum Sent {
    Object(SubgroupPart),
    Datagram(moqt::object::Object),
    Reprioritized(moqt::object::StreamPriority),
    Closed,
    Reset(DataStreamResetCode),
}

/// The first value a draft-14 varint cannot carry.
pub(crate) const DRAFT14_VARINT_LIMIT: u64 = 1 << 62;

/// Fails, as moqt's draft-14 encoder does, on a Group ID, Subgroup ID or
/// Object ID of 2^62 or more, or extension headers draft-14 cannot carry.
fn refuse_as_draft14(part: &SubgroupPart) -> anyhow::Result<()> {
    let object = match part {
        SubgroupPart::Header(start) => {
            refuse_location_as_draft14(start.group_id, 0)?;
            return refuse_subgroup_id_as_draft14(start.subgroup_id);
        }
        SubgroupPart::Object(object) | SubgroupPart::ObjectStart { object, .. } => object,
        SubgroupPart::Payload { .. } => return Ok(()),
    };
    refuse_object_as_draft14(object)
}

fn refuse_object_as_draft14(object: &moqt::object::Object) -> anyhow::Result<()> {
    refuse_location_as_draft14(object.location.group_id, object.location.object_id)?;
    object.extension_headers.encode()?;
    Ok(())
}

fn refuse_subgroup_id_as_draft14(subgroup_id: u64) -> anyhow::Result<()> {
    if subgroup_id >= DRAFT14_VARINT_LIMIT {
        return Err(UnencodableObject::SubgroupId { subgroup_id }.into());
    }
    Ok(())
}

fn refuse_location_as_draft14(group_id: u64, object_id: u64) -> anyhow::Result<()> {
    if group_id >= DRAFT14_VARINT_LIMIT || object_id >= DRAFT14_VARINT_LIMIT {
        return Err(UnencodableObject::Location {
            group_id,
            object_id,
        }
        .into());
    }
    Ok(())
}

struct MockDataSender {
    sent: mpsc::UnboundedSender<Sent>,
    stall_writes: bool,
    withhold_acknowledgements: bool,
    refuses_draft14_values: bool,
}

#[async_trait::async_trait]
impl DataSender for MockDataSender {
    async fn send_object(&mut self, object: SubgroupPart) -> anyhow::Result<()> {
        if self.stall_writes && !matches!(object, SubgroupPart::Header(_)) {
            std::future::pending::<()>().await
        }
        if self.refuses_draft14_values {
            refuse_as_draft14(&object)?;
        }
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

    async fn acknowledged(&mut self) {
        if self.withhold_acknowledgements {
            std::future::pending::<()>().await
        }
    }
}

/// Takes a datagram whose payload is at most `payload_limit` bytes.
struct MockDatagramSender {
    sent: mpsc::UnboundedSender<Sent>,
    payload_limit: usize,
    refuses_draft14_values: bool,
}

#[async_trait::async_trait]
impl DatagramSender for MockDatagramSender {
    async fn send(&mut self, object: moqt::object::Object) -> anyhow::Result<DatagramDelivery> {
        if self.refuses_draft14_values {
            refuse_object_as_draft14(&object)?;
        }
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
    stall_writes: bool,
    withhold_acknowledgements: bool,
    refuses_draft14_values: bool,
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
            stall_writes: self.stall_writes,
            withhold_acknowledgements: self.withhold_acknowledgements,
            refuses_draft14_values: self.refuses_draft14_values,
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
    stall_writes: bool,
    withhold_acknowledgements: bool,
    mixes_forwarding_preferences: bool,
    datagram_payload_limit: usize,
    refuses_draft14_values: bool,
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
                stall_writes: false,
                withhold_acknowledgements: false,
                mixes_forwarding_preferences,
                datagram_payload_limit: usize::MAX,
                refuses_draft14_values: false,
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

    /// A subscriber whose streams take a header and then no more: each
    /// object write waits forever, as it does on a full send buffer.
    pub(crate) fn stalling_writes(self) -> Self {
        Self {
            stall_writes: true,
            ..self
        }
    }

    /// A subscriber that never acknowledges a closed stream's data.
    pub(crate) fn withholding_acknowledgements(self) -> Self {
        Self {
            withhold_acknowledgements: true,
            ..self
        }
    }

    /// A draft-14 subscriber: a send fails, as moqt's does, on a value of
    /// 2^62 or more.
    pub(crate) fn refusing_draft14_values(self) -> Self {
        Self {
            refuses_draft14_values: true,
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
            stall_writes: self.stall_writes,
            withhold_acknowledgements: self.withhold_acknowledgements,
            refuses_draft14_values: self.refuses_draft14_values,
        })
    }

    fn new_datagram(
        &self,
        _downstream_subscription: &DownstreamSubscription,
    ) -> Box<dyn DatagramSender> {
        Box::new(MockDatagramSender {
            sent: self.sent.clone(),
            payload_limit: self.datagram_payload_limit,
            refuses_draft14_values: self.refuses_draft14_values,
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
