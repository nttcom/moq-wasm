use std::{
    pin::Pin,
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, ReadBuf};

use crate::modules::transport::transport_receive_stream::TransportReceiveStream;

#[derive(Debug)]
pub(crate) struct Reader<S: TransportReceiveStream> {
    receive_stream: S,
}

impl<S: TransportReceiveStream> Reader<S> {
    pub(crate) fn new(receive_stream: S) -> Self {
        Self { receive_stream }
    }
}

impl<S: TransportReceiveStream> AsyncRead for Reader<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.receive_stream
            .poll_read(cx, buf)
            .map_err(|e| std::io::Error::other(format!("read error: {:?}", e)))
    }
}

#[cfg(test)]
mod tests {
    use std::task::Poll;

    use tokio::io::AsyncReadExt;

    use super::Reader;
    use crate::modules::transport::{
        read_error::ReadError, transport_receive_stream::MockTransportReceiveStream,
    };

    #[tokio::test]
    async fn read_writes_received_bytes_into_caller_buffer() {
        // Arrange
        let mut receive_stream = MockTransportReceiveStream::new();
        receive_stream.expect_poll_read().returning(|_, buf| {
            buf.put_slice(b"abc");
            Poll::Ready(Ok(()))
        });
        let mut reader = Reader::new(receive_stream);
        let mut out = [0u8; 8];

        // Act
        let size = reader.read(&mut out).await.unwrap();

        // Assert
        assert_eq!(&out[..size], b"abc");
    }

    #[tokio::test]
    async fn read_wraps_transport_error_in_io_error() {
        // Arrange
        let mut receive_stream = MockTransportReceiveStream::new();
        receive_stream
            .expect_poll_read()
            .returning(|_, _| Poll::Ready(Err(ReadError::Closed)));
        let mut reader = Reader::new(receive_stream);
        let mut out = [0u8; 8];

        // Act
        let error = reader.read(&mut out).await.unwrap_err();

        // Assert
        assert_eq!(error.to_string(), "read error: Closed");
    }
}
