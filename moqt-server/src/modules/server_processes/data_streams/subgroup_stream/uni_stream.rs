use wtransport::{
    error::{StreamReadError, StreamWriteError},
    RecvStream, SendStream, VarInt,
};

pub(crate) struct UniRecvStream {
    stable_id: usize,
    stream_id: u64,
    recv_stream: RecvStream,
}

impl UniRecvStream {
    pub(crate) fn new(stable_id: usize, stream_id: u64, recv_stream: RecvStream) -> UniRecvStream {
        UniRecvStream {
            stable_id,
            stream_id,
            recv_stream,
        }
    }

    pub(crate) fn stable_id(&self) -> usize {
        self.stable_id
    }

    pub(crate) fn stream_id(&self) -> u64 {
        self.stream_id
    }

    pub(crate) async fn read(
        &mut self,
        buffer: &mut [u8],
    ) -> Result<Option<usize>, StreamReadError> {
        self.recv_stream.read(buffer).await
    }

    pub(crate) fn stop(self) {
        // Use code 0 for normal termination
        // TODO: Use accurate error code
        let code = VarInt::from_u32(0);
        self.recv_stream.stop(code);
    }
}

pub(crate) struct UniSendStream {
    stable_id: usize,
    stream_id: u64,
    send_stream: SendStream,
}

impl UniSendStream {
    pub fn new(stable_id: usize, stream_id: u64, send_stream: SendStream) -> UniSendStream {
        UniSendStream {
            stable_id,
            stream_id,
            send_stream,
        }
    }

    pub(crate) fn stable_id(&self) -> usize {
        self.stable_id
    }

    pub(crate) fn stream_id(&self) -> u64 {
        self.stream_id
    }

    pub(crate) async fn write_all(&mut self, buffer: &[u8]) -> Result<(), StreamWriteError> {
        self.send_stream.write_all(buffer).await
    }

    pub(crate) async fn finish(&mut self) -> Result<(), StreamWriteError> {
        self.send_stream.finish().await
    }
}
