use std::{
    pin::Pin,
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, ReadBuf};

use crate::modules::transport::transport_connection::BoxedReceiveStream;

#[derive(Debug)]
pub(crate) struct Reader {
    receive_stream: BoxedReceiveStream,
}

impl Reader {
    pub(crate) fn new(receive_stream: BoxedReceiveStream) -> Self {
        Self { receive_stream }
    }
}

impl AsyncRead for Reader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.receive_stream
            .poll_read(cx, buf)
            .map_err(std::io::Error::other)
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
        let mut reader = Reader::new(Box::new(receive_stream));
        let mut out = [0u8; 8];

        // Act
        let size = reader.read(&mut out).await.unwrap();

        // Assert
        assert_eq!(&out[..size], b"abc");
    }

    #[tokio::test]
    async fn read_keeps_the_transport_error_inside_the_io_error() {
        // Arrange
        let mut receive_stream = MockTransportReceiveStream::new();
        receive_stream
            .expect_poll_read()
            .returning(|_, _| Poll::Ready(Err(ReadError::Closed)));
        let mut reader = Reader::new(Box::new(receive_stream));
        let mut out = [0u8; 8];

        // Act
        let error = reader.read(&mut out).await.unwrap_err();

        // Assert
        assert_eq!(
            error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<ReadError>()),
            Some(&ReadError::Closed)
        );
    }
}
