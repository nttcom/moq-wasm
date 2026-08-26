use tokio::sync::mpsc;

use crate::modules::core::{
    data_object::DataObject,
    data_sender::{
        DataSender, fetch_sender::FetchSender, stream_sender_factory::StreamSenderFactory,
    },
    publisher::Publisher,
    subscription::DownstreamSubscription,
};

struct MockDataSender {
    sent: mpsc::UnboundedSender<Option<DataObject>>,
}

#[async_trait::async_trait]
impl DataSender for MockDataSender {
    async fn send_object(&mut self, object: DataObject) -> anyhow::Result<()> {
        self.sent
            .send(Some(object))
            .map_err(|_| anyhow::anyhow!("subscriber side dropped"))
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        self.sent
            .send(None)
            .map_err(|_| anyhow::anyhow!("subscriber side dropped"))
    }
}

struct MockStreamSenderFactory {
    sent: mpsc::UnboundedSender<Option<DataObject>>,
}

#[async_trait::async_trait]
impl StreamSenderFactory for MockStreamSenderFactory {
    async fn next(&mut self) -> anyhow::Result<Box<dyn DataSender>> {
        Ok(Box::new(MockDataSender {
            sent: self.sent.clone(),
        }))
    }
}

/// A PUBLISH_DONE observed by the mock downstream publisher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SentPublishDone {
    pub(crate) request_id: u64,
    pub(crate) status_code: u64,
    pub(crate) stream_count: u64,
    pub(crate) error_reason: String,
}

pub(crate) struct MockPublisher {
    sent: mpsc::UnboundedSender<Option<DataObject>>,
    publish_done: mpsc::UnboundedSender<SentPublishDone>,
}

pub(crate) struct MockPublisherObservers {
    pub(crate) sent: mpsc::UnboundedReceiver<Option<DataObject>>,
    pub(crate) publish_done: mpsc::UnboundedReceiver<SentPublishDone>,
}

impl MockPublisher {
    pub(crate) fn channel() -> (Self, MockPublisherObservers) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let (publish_done_sender, publish_done_receiver) = mpsc::unbounded_channel();
        (
            Self {
                sent: sender,
                publish_done: publish_done_sender,
            },
            MockPublisherObservers {
                sent: receiver,
                publish_done: publish_done_receiver,
            },
        )
    }
}

#[async_trait::async_trait]
impl Publisher for MockPublisher {
    async fn send_publish_namespace(&self, _namespaces: String) -> anyhow::Result<()> {
        unreachable!("not used by the egress path under test")
    }

    async fn send_publish_namespace_done(&self, _namespace: String) -> anyhow::Result<()> {
        unreachable!("not used by the egress path under test")
    }

    async fn send_publish(
        &self,
        _track_namespace: String,
        _track_name: String,
    ) -> anyhow::Result<DownstreamSubscription> {
        unreachable!("not used by the egress path under test")
    }

    async fn send_publish_done(
        &self,
        request_id: u64,
        status_code: u64,
        stream_count: u64,
        error_reason: String,
    ) -> anyhow::Result<()> {
        self.publish_done
            .send(SentPublishDone {
                request_id,
                status_code,
                stream_count,
                error_reason,
            })
            .map_err(|_| anyhow::anyhow!("test side dropped"))
    }

    fn new_stream_factory(
        &self,
        _downstream_subscription: &DownstreamSubscription,
    ) -> Box<dyn StreamSenderFactory> {
        Box::new(MockStreamSenderFactory {
            sent: self.sent.clone(),
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
