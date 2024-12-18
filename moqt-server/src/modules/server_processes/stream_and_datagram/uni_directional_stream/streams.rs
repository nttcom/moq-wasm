use std::sync::Arc;
use tokio::sync::Mutex;
use wtransport::{error::StreamReadError, RecvStream, SendStream};

pub(crate) struct UniRecvStream {
    stable_id: usize,
    stream_id: u64,
    recv_stream: Arc<Mutex<RecvStream>>,
}

impl UniRecvStream {
    pub fn new(
        stable_id: usize,
        stream_id: u64,
        recv_stream: Arc<Mutex<RecvStream>>,
    ) -> UniRecvStream {
        UniRecvStream {
            stable_id,
            stream_id,
            recv_stream,
        }
    }

    pub fn stable_id(&self) -> usize {
        self.stable_id
    }

    pub fn stream_id(&self) -> u64 {
        self.stream_id
    }

    pub async fn read(&mut self, buffer: &mut [u8]) -> Result<Option<usize>, StreamReadError> {
        let mut recv_stream = self.recv_stream.lock().await;
        recv_stream.read(buffer).await
    }
}

pub(crate) struct UniSendStream {
    stable_id: usize,
    stream_id: u64,
    subscribe_id: u64,
    pub(crate) send_stream: SendStream,
}

impl UniSendStream {
    pub fn new(
        stable_id: usize,
        stream_id: u64,
        subscribe_id: u64,
        send_stream: SendStream,
    ) -> UniSendStream {
        UniSendStream {
            stable_id,
            stream_id,
            subscribe_id,
            send_stream,
        }
    }

    pub fn stable_id(&self) -> usize {
        self.stable_id
    }

    pub fn stream_id(&self) -> u64 {
        self.stream_id
    }

    pub fn subscribe_id(&self) -> u64 {
        self.subscribe_id
    }
}
