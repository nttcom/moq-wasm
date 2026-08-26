use tokio::sync::mpsc;

use crate::modules::{
    core::{data_object::DataObject, data_receiver::stream_receiver::StreamReceiver},
    relay::tests::harness::fixtures::data_object::{
        make_header, make_payload_object, ordered_payload,
    },
};

struct MockStreamReceiver {
    receiver: mpsc::UnboundedReceiver<Result<Option<DataObject>, moqt::StreamReceiveError>>,
}

#[async_trait::async_trait]
impl StreamReceiver for MockStreamReceiver {
    async fn receive_object(&mut self) -> Result<Option<DataObject>, moqt::StreamReceiveError> {
        match self.receiver.recv().await {
            Some(item) => item,
            None => Ok(None),
        }
    }
}

pub(crate) struct UpstreamSubgroupStream {
    sender: mpsc::UnboundedSender<Result<Option<DataObject>, moqt::StreamReceiveError>>,
}

impl UpstreamSubgroupStream {
    pub(crate) fn open() -> (Self, Box<dyn StreamReceiver>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, Box::new(MockStreamReceiver { receiver }))
    }

    pub(crate) fn header(&self, group_id: u64) {
        self.sender
            .send(Ok(Some(make_header(group_id))))
            .expect("ingress should be reading this stream");
    }

    pub(crate) fn object(&self, index: usize) {
        self.object_with_payload(index, ordered_payload(index));
    }

    pub(crate) fn object_with_payload(&self, index: usize, payload: bytes::Bytes) {
        let delta = if index == 0 { 0 } else { 1 };
        self.sender
            .send(Ok(Some(make_payload_object(delta, payload))))
            .expect("ingress should be reading this stream");
    }

    pub(crate) fn fin(&self) {
        self.sender
            .send(Ok(None))
            .expect("ingress should be reading this stream");
    }
}
