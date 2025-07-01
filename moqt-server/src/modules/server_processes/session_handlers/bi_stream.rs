use std::sync::Arc;
use tokio::sync::Mutex;
use wtransport::quinn;

pub(crate) trait BiStreamTrait {}

pub(crate) struct QuicBiStream {
    pub(crate) stable_id: usize,
    pub(crate) stream_id: u64,
    pub(crate) recv_stream: quinn::RecvStream,
    pub(crate) shared_send_stream: Arc<Mutex<quinn::SendStream>>,
}

impl BiStreamTrait for QuicBiStream {}

impl QuicBiStream {
    pub(super) fn new(
        stable_id: usize,
        stream_id: u64,
        recv_stream: quinn::RecvStream,
        send_stream: Arc<Mutex<quinn::SendStream>>,
    ) -> Self {
        Self { stable_id, stream_id, recv_stream, shared_send_stream: send_stream }
    }
}
