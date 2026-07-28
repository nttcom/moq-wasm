//! The publisher-client role: plays one upstream subgroup stream toward the
//! ingress through the same `StreamReceiver` seam the QUIC transport
//! implements, so tests control exactly when each object (and the FIN)
//! becomes visible to the relay.

use tokio::sync::mpsc;

use crate::modules::{
    core::{data_object::DataObject, data_receiver::stream_receiver::StreamReceiver},
    relay::tests::fixtures::{make_header, make_payload_object, ordered_payload},
};

type FeedItem = Result<Option<DataObject>, moqt::StreamReceiveError>;

struct ChannelStreamReceiver {
    receiver: mpsc::UnboundedReceiver<FeedItem>,
}

#[async_trait::async_trait]
impl StreamReceiver for ChannelStreamReceiver {
    async fn receive_object(&mut self) -> Result<Option<DataObject>, moqt::StreamReceiveError> {
        match self.receiver.recv().await {
            Some(item) => item,
            // Dropped feed sender behaves like a FIN.
            None => Ok(None),
        }
    }
}

/// Publisher-side handle of one upstream subgroup stream.
pub(crate) struct UpstreamSubgroupStream {
    sender: mpsc::UnboundedSender<FeedItem>,
}

impl UpstreamSubgroupStream {
    /// Opens the stream: the handle plays the publisher, the receiver is
    /// what the ingress reads from.
    pub(crate) fn open() -> (Self, Box<dyn StreamReceiver>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (
            Self { sender },
            Box::new(ChannelStreamReceiver { receiver }),
        )
    }

    pub(crate) fn header(&self, group_id: u64) {
        self.sender
            .send(Ok(Some(make_header(group_id))))
            .expect("ingress feed should be open");
    }

    pub(crate) fn object(&self, index: usize) {
        let delta = if index == 0 { 0 } else { 1 };
        self.sender
            .send(Ok(Some(make_payload_object(delta, ordered_payload(index)))))
            .expect("ingress feed should be open");
    }

    pub(crate) fn fin(&self) {
        self.sender
            .send(Ok(None))
            .expect("ingress feed should be open");
    }
}
