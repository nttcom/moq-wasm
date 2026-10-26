use std::sync::Mutex;

use moqt::object::Object;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::{
    session::{
        data_object::DataObject,
        data_receiver::{
            receiver::{DataReceiver, UpstreamData},
            stream_receiver::StreamReceiver,
        },
    },
    test_support::relay_harness::{
        RECV_TIMEOUT,
        fixtures::data_object::{make_header, make_payload_object, ordered_payload},
    },
};

type ReceiveResult = Result<Option<DataObject>, moqt::object::StreamReceiveError>;

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

/// Feeds a stream reader the objects moqt would hand it: the header, then
/// objects whose ids follow the previous one unless a gap is asked for.
pub(crate) struct UpstreamSubgroupStream {
    sender: mpsc::UnboundedSender<ReceiveResult>,
    reader: JoinHandle<()>,
    /// The group of the header sent and the next object id.
    start: Mutex<Option<(u64, u64)>>,
}

impl UpstreamSubgroupStream {
    pub(crate) fn open(
        spawn_reader: impl FnOnce(Box<dyn StreamReceiver>) -> JoinHandle<()>,
    ) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let reader = spawn_reader(Box::new(MockStreamReceiver { receiver }));
        Self {
            sender,
            reader,
            start: Mutex::new(None),
        }
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
        *self.start.lock().unwrap() = Some((group_id, 0));
        self.send(make_header(group_id));
    }

    pub(crate) fn object(&self, index: usize) {
        self.object_with_gap(0, index);
    }

    /// Skips `gap` object ids before this object.
    pub(crate) fn object_with_gap(&self, gap: u64, index: usize) {
        self.send_payload(gap, ordered_payload(index));
    }

    pub(crate) fn object_with_payload(&self, payload: bytes::Bytes) {
        self.send_payload(0, payload);
    }

    fn send_payload(&self, gap: u64, payload: bytes::Bytes) {
        let (group_id, object_id) = {
            let mut start = self.start.lock().unwrap();
            let (group_id, next_object_id) = start.as_mut().expect("send the header first");
            let object_id = *next_object_id + gap;
            *next_object_id = object_id + 1;
            (*group_id, object_id)
        };
        self.send(make_payload_object(group_id, object_id, payload));
    }

    pub(crate) fn reset(&self) {
        self.push(Err(moqt::object::StreamReceiveError::Closed(
            moqt::object::ReadError::Reset,
        )));
    }

    pub(crate) fn decode_error(&self) {
        self.push(Err(moqt::object::StreamReceiveError::Decode(
            "malformed object field".to_string(),
        )));
    }

    pub(crate) fn protocol_violation(&self) {
        self.push(Err(moqt::object::StreamReceiveError::ProtocolViolation(
            "invalid object status 0x2".to_string(),
        )));
    }

    /// moqt found the track malformed reading the stream.
    pub(crate) fn malformed_track(&self, error: moqt::object::MalformedTrackError) {
        self.push(Err(moqt::object::StreamReceiveError::MalformedTrack(error)));
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

/// Feeds an ingest what moqt's `DataReceiver` would hand it: the subgroup
/// streams and datagrams of one upstream subscription, which ends when this
/// is dropped.
pub(crate) struct UpstreamTrack {
    sender: mpsc::UnboundedSender<UpstreamData>,
}

struct ChannelDataReceiver {
    receiver: mpsc::UnboundedReceiver<UpstreamData>,
}

#[async_trait::async_trait]
impl DataReceiver for ChannelDataReceiver {
    async fn next(&mut self) -> anyhow::Result<UpstreamData> {
        self.receiver
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("the subscription ended"))
    }
}

impl UpstreamTrack {
    pub(crate) fn channel() -> (Self, Box<dyn DataReceiver>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, Box::new(ChannelDataReceiver { receiver }))
    }

    pub(crate) fn datagram(&self, object: Object) {
        self.sender
            .send(UpstreamData::Datagram(object))
            .expect("ingress should be reading this track");
    }

    /// The ingest reads the stream in a task of its own, so the returned
    /// stream's `wait_reader_end` resolves at once.
    pub(crate) fn open_stream(&self) -> UpstreamSubgroupStream {
        UpstreamSubgroupStream::open(|receiver| {
            self.sender
                .send(UpstreamData::Subgroup(receiver))
                .expect("ingress should be reading this track");
            tokio::spawn(async {})
        })
    }
}

pub(crate) struct FailingUpstream(pub(crate) fn() -> anyhow::Error);

#[async_trait::async_trait]
impl DataReceiver for FailingUpstream {
    async fn next(&mut self) -> anyhow::Result<UpstreamData> {
        Err(self.0())
    }
}
