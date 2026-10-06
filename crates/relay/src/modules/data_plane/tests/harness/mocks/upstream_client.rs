use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::{
    core::{data_object::DataObject, data_receiver::stream_receiver::StreamReceiver},
    data_plane::tests::harness::{
        RECV_TIMEOUT,
        fixtures::data_object::{make_header, make_payload_object, ordered_payload},
    },
};

type ReceiveResult = Result<Option<DataObject>, moqt::StreamReceiveError>;

struct MockStreamReceiver {
    receiver: mpsc::UnboundedReceiver<ReceiveResult>,
}

#[async_trait::async_trait]
impl StreamReceiver for MockStreamReceiver {
    async fn receive_object(&mut self) -> ReceiveResult {
        match self.receiver.recv().await {
            Some(item) => item,
            None => Ok(None),
        }
    }
}

pub(crate) struct UpstreamSubgroupStream {
    sender: mpsc::UnboundedSender<ReceiveResult>,
    reader: JoinHandle<()>,
}

impl UpstreamSubgroupStream {
    pub(crate) fn open(
        spawn_reader: impl FnOnce(Box<dyn StreamReceiver>) -> JoinHandle<()>,
    ) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let reader = spawn_reader(Box::new(MockStreamReceiver { receiver }));
        Self { sender, reader }
    }

    fn push(&self, item: ReceiveResult) {
        self.sender
            .send(item)
            .expect("ingress should be reading this stream");
    }

    pub(crate) fn send(&self, object: DataObject) {
        self.push(Ok(Some(object)));
    }

    pub(crate) fn header(&self, group_id: u64) {
        self.send(make_header(group_id));
    }

    pub(crate) fn object(&self, index: usize) {
        self.object_with_payload(ordered_payload(index));
    }

    pub(crate) fn object_with_delta(&self, object_id_delta: u64, index: usize) {
        self.send(make_payload_object(object_id_delta, ordered_payload(index)));
    }

    pub(crate) fn object_with_payload(&self, payload: bytes::Bytes) {
        self.send(make_payload_object(0, payload));
    }

    pub(crate) fn reset(&self) {
        self.push(Err(moqt::StreamReceiveError::Closed(
            "stream reset by peer".to_string(),
        )));
    }

    pub(crate) fn decode_error(&self) {
        self.push(Err(moqt::StreamReceiveError::Decode(
            "malformed object field".to_string(),
        )));
    }

    pub(crate) fn fin(&self) {
        self.push(Ok(None));
    }

    pub(crate) async fn wait_reader_end(&mut self) {
        tokio::time::timeout(RECV_TIMEOUT, &mut self.reader)
            .await
            .expect("stream reader should end")
            .expect("stream reader should not panic");
    }
}
