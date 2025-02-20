use std::sync::Arc;
use tokio::sync::Mutex;
use wtransport::{RecvStream, SendStream};

pub(crate) struct BiStream {
    stable_id: usize,
    stream_id: u64,
    pub(crate) recv_stream: RecvStream,
    pub(crate) shared_send_stream: Arc<Mutex<SendStream>>,
}

impl BiStream {
    pub fn new(
        stable_id: usize,
        stream_id: u64,
        recv_stream: RecvStream,
        shared_send_stream: Arc<Mutex<SendStream>>,
    ) -> BiStream {
        BiStream {
            stable_id,
            stream_id,
            recv_stream,
            shared_send_stream,
        }
    }

    pub fn stable_id(&self) -> usize {
        self.stable_id
    }

    pub fn stream_id(&self) -> u64 {
        self.stream_id
    }
}
