//! A message transport over channels, for testing QMux without a socket.

use std::task::Poll;

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures::future::poll_fn;
use tokio::{io::ReadBuf, sync::mpsc};

use super::{
    frame::Frame,
    message_transport::{MessageSink, MessageSource},
    qmux_connection::QmuxConnection,
    stream_id::Role,
    transport_parameters::TransportParameters,
};
use crate::modules::transport::{
    read_error::ReadError, transport_addresses::TransportAddresses,
    transport_connection::BoxedReceiveStream,
};

pub(crate) struct InMemorySink {
    sender: Option<mpsc::UnboundedSender<Bytes>>,
}

pub(crate) struct InMemorySource {
    receiver: mpsc::UnboundedReceiver<Bytes>,
}

#[async_trait]
impl MessageSink for InMemorySink {
    async fn send(&mut self, message: Bytes) -> anyhow::Result<()> {
        self.sender
            .as_ref()
            .and_then(|sender| sender.send(message).ok())
            .ok_or_else(|| anyhow::anyhow!("in-memory transport closed"))
    }

    async fn close(&mut self, _reason: &str) -> anyhow::Result<()> {
        self.sender = None;
        Ok(())
    }
}

#[async_trait]
impl MessageSource for InMemorySource {
    async fn receive(&mut self) -> anyhow::Result<Option<Bytes>> {
        Ok(self.receiver.recv().await)
    }
}

/// Two connected ends.
pub(crate) fn link() -> (
    (InMemorySink, InMemorySource),
    (InMemorySink, InMemorySource),
) {
    let (to_b, from_a) = mpsc::unbounded_channel();
    let (to_a, from_b) = mpsc::unbounded_channel();
    (
        (
            InMemorySink { sender: Some(to_b) },
            InMemorySource { receiver: from_b },
        ),
        (
            InMemorySink { sender: Some(to_a) },
            InMemorySource { receiver: from_a },
        ),
    )
}

/// An established (client, server) pair.
pub(crate) async fn connected() -> (QmuxConnection, QmuxConnection) {
    let ((client_sink, client_source), (server_sink, server_source)) = link();
    let client = QmuxConnection::establish(
        client_sink,
        client_source,
        Role::Client,
        Some("moq-00".into()),
        TransportAddresses::default(),
    );
    let server = QmuxConnection::establish(
        server_sink,
        server_source,
        Role::Server,
        Some("moq-00".into()),
        TransportAddresses::default(),
    );
    let (client, server) = tokio::join!(client, server);
    (client.unwrap(), server.unwrap())
}

/// A client whose peer is the test itself: the returned sink and source are
/// the server's ends, and the server's transport parameters are already sent.
pub(crate) async fn client_with_raw_peer(
    peer_parameters: TransportParameters,
) -> (QmuxConnection, InMemorySink, InMemorySource) {
    let ((client_sink, client_source), (mut server_sink, mut server_source)) = link();
    let mut record = BytesMut::new();
    Frame::TransportParameters(peer_parameters).encode(&mut record);
    server_sink.send(record.freeze()).await.unwrap();
    let client = QmuxConnection::establish(
        client_sink,
        client_source,
        Role::Client,
        None,
        TransportAddresses::default(),
    )
    .await
    .unwrap();
    let first = server_source.receive().await.unwrap().unwrap();
    assert!(matches!(
        Frame::decode_record(first).unwrap().as_slice(),
        [Frame::TransportParameters(_)]
    ));
    (client, server_sink, server_source)
}

pub(crate) async fn read_to_end(stream: &mut BoxedReceiveStream) -> Result<Vec<u8>, ReadError> {
    let mut out = Vec::new();
    let mut buffer = vec![0u8; 8192];
    loop {
        let read = poll_fn(|cx| {
            let mut read_buf = ReadBuf::new(&mut buffer);
            match stream.poll_read(cx, &mut read_buf) {
                Poll::Ready(Ok(())) => Poll::Ready(Ok(read_buf.filled().len())),
                Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
                Poll::Pending => Poll::Pending,
            }
        })
        .await?;
        if read == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&buffer[..read]);
    }
}
