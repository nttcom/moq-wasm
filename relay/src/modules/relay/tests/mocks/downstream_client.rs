//! The subscriber-client role: receives whatever the egress sends through
//! the same `Publisher`/`DataSender` seams the QUIC transport implements,
//! recording every object tagged with the downstream uni-stream it was sent
//! on.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::modules::core::{
    data_object::DataObject,
    data_sender::{
        DataSender, fetch_sender::FetchSender, stream_sender_factory::StreamSenderFactory,
    },
    publisher::Publisher,
    subscription::DownstreamSubscription,
};

#[derive(Debug)]
pub(crate) enum EgressEvent {
    Object { stream: usize, object: DataObject },
    Closed { stream: usize },
}

struct MockDataSender {
    stream: usize,
    events: mpsc::UnboundedSender<EgressEvent>,
}

#[async_trait::async_trait]
impl DataSender for MockDataSender {
    async fn send_object(&mut self, object: DataObject) -> anyhow::Result<()> {
        self.events
            .send(EgressEvent::Object {
                stream: self.stream,
                object,
            })
            .map_err(|_| anyhow::anyhow!("egress capture receiver dropped"))
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        self.events
            .send(EgressEvent::Closed {
                stream: self.stream,
            })
            .map_err(|_| anyhow::anyhow!("egress capture receiver dropped"))
    }
}

struct MockStreamSenderFactory {
    next_stream: Arc<AtomicUsize>,
    events: mpsc::UnboundedSender<EgressEvent>,
}

#[async_trait::async_trait]
impl StreamSenderFactory for MockStreamSenderFactory {
    async fn next(&mut self) -> anyhow::Result<Box<dyn DataSender>> {
        let stream = self.next_stream.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(MockDataSender {
            stream,
            events: self.events.clone(),
        }))
    }
}

/// The relay-side sending handle toward the subscriber session
/// (`core::publisher::Publisher`), recording instead of sending.
pub(crate) struct MockPublisher {
    next_stream: Arc<AtomicUsize>,
    events: mpsc::UnboundedSender<EgressEvent>,
}

impl MockPublisher {
    /// The publisher records into the returned receiver.
    pub(crate) fn channel() -> (Self, mpsc::UnboundedReceiver<EgressEvent>) {
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        (
            Self {
                next_stream: Arc::new(AtomicUsize::new(0)),
                events: event_sender,
            },
            event_receiver,
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

    fn new_stream_factory(
        &self,
        _downstream_subscription: &DownstreamSubscription,
    ) -> Box<dyn StreamSenderFactory> {
        Box::new(MockStreamSenderFactory {
            next_stream: self.next_stream.clone(),
            events: self.events.clone(),
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

// ---------------------------------------------------------------------------
// Queries over the recorded events
// ---------------------------------------------------------------------------

/// Payloads sent on one downstream stream, in send order.
pub(crate) fn payloads_on_stream(events: &[EgressEvent], stream: usize) -> Vec<Bytes> {
    events
        .iter()
        .filter_map(|event| match event {
            EgressEvent::Object {
                stream: s,
                object: DataObject::SubgroupObject(field),
            } if *s == stream => match &field.subgroup_object {
                moqt::SubgroupObject::Payload { data, .. } => Some(data.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

pub(crate) fn stream_closed(events: &[EgressEvent], stream: usize) -> bool {
    events
        .iter()
        .any(|event| matches!(event, EgressEvent::Closed { stream: s } if *s == stream))
}

pub(crate) fn header_group_on_stream(events: &[EgressEvent], stream: usize) -> Option<u64> {
    events.iter().find_map(|event| match event {
        EgressEvent::Object {
            stream: s,
            object: DataObject::SubgroupHeader(header),
        } if *s == stream => Some(header.group_id),
        _ => None,
    })
}
