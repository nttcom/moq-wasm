use crate::modules::transport::transport_connection::{BoxedReceiveStream, BoxedSendStream};
use async_trait::async_trait;
use futures::{FutureExt, future::BoxFuture};
use std::{
    sync::PoisonError,
    task::{Context, Poll, Waker},
};

use super::wt_receive_stream::WtReceiveStream;
use super::wt_send_stream::WtSendStream;
use crate::modules::transport::application_protocol::NegotiatedProtocol;
use crate::modules::transport::transport_addresses::TransportAddresses;
use crate::modules::transport::transport_connection::{TransportClose, TransportConnection};
use crate::modules::transport::transport_stats::TransportStats;

type OpeningBi = BoxFuture<
    'static,
    Result<
        (
            web_transport_quinn::SendStream,
            web_transport_quinn::RecvStream,
        ),
        web_transport_quinn::SessionError,
    >,
>;

pub struct WtConnection {
    session: web_transport_quinn::Session,
    /// An open `try_open_bi` left pending. After taking a stream ID it still
    /// writes the stream header, so dropping it then would strand the
    /// stream; the next open resumes it instead.
    opening_bi: std::sync::Mutex<Option<OpeningBi>>,
}

impl std::fmt::Debug for WtConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WtConnection")
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl WtConnection {
    pub(crate) fn new(session: web_transport_quinn::Session) -> Self {
        Self {
            session,
            opening_bi: std::sync::Mutex::new(None),
        }
    }

    fn take_opening_bi(&self) -> Option<OpeningBi> {
        self.opening_bi
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

fn boxed_bi(
    (send, recv): (
        web_transport_quinn::SendStream,
        web_transport_quinn::RecvStream,
    ),
) -> (BoxedSendStream, BoxedReceiveStream) {
    (
        Box::new(WtSendStream { send_stream: send }),
        Box::new(WtReceiveStream { recv_stream: recv }),
    )
}

#[async_trait]
impl TransportConnection for WtConnection {
    async fn closed(&self) -> TransportClose {
        let error = self.session.closed().await;
        tracing::info!("WebTransport connection closed: {:?}", error);
        match error {
            web_transport_quinn::SessionError::WebTransportError(
                web_transport_quinn::WebTransportError::Closed(code, reason),
            ) => TransportClose {
                code: Some(code),
                reason,
            },
            other => TransportClose {
                code: None,
                reason: other.to_string(),
            },
        }
    }

    fn close(&self, code: u32, reason: &str) {
        self.session.close(code, reason.as_bytes());
        tracing::info!(code, reason, "WebTransport connection close requested");
    }

    async fn open_bi(&self) -> anyhow::Result<(BoxedSendStream, BoxedReceiveStream)> {
        let opened = match self.take_opening_bi() {
            Some(opening) => opening.await?,
            None => self.session.open_bi().await?,
        };
        Ok(boxed_bi(opened))
    }

    async fn try_open_bi(&self) -> anyhow::Result<Option<(BoxedSendStream, BoxedReceiveStream)>> {
        let mut opening = self.take_opening_bi().unwrap_or_else(|| {
            let session = self.session.clone();
            async move { session.open_bi().await }.boxed()
        });
        match opening.poll_unpin(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(opened) => Ok(Some(boxed_bi(opened?))),
            Poll::Pending => {
                *self
                    .opening_bi
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(opening);
                Ok(None)
            }
        }
    }

    async fn accept_bi(&self) -> anyhow::Result<(BoxedSendStream, BoxedReceiveStream)> {
        let (send, recv) = self.session.accept_bi().await?;
        Ok((
            Box::new(WtSendStream { send_stream: send }),
            Box::new(WtReceiveStream { recv_stream: recv }),
        ))
    }

    async fn open_uni(&self) -> anyhow::Result<BoxedSendStream> {
        let send = self.session.open_uni().await?;
        Ok(Box::new(WtSendStream { send_stream: send }))
    }

    async fn accept_uni(&self) -> anyhow::Result<BoxedReceiveStream> {
        let recv = self.session.accept_uni().await?;
        Ok(Box::new(WtReceiveStream { recv_stream: recv }))
    }

    fn send_datagram(&self, bytes: bytes::BytesMut) -> anyhow::Result<()> {
        let len = bytes.len();
        match self.session.send_datagram(bytes.freeze()) {
            Err(web_transport_quinn::SessionError::SendDatagramError(
                quinn::SendDatagramError::TooLarge,
            )) => {
                tracing::debug!(len, "dropping a datagram larger than the session allows");
                Ok(())
            }
            result => Ok(result?),
        }
    }

    async fn receive_datagram(&self) -> anyhow::Result<bytes::BytesMut> {
        let datagram = self.session.read_datagram().await?;
        Ok(bytes::BytesMut::from(&datagram[..]))
    }

    fn stats(&self) -> TransportStats {
        quinn::Connection::stats(&self.session).into()
    }

    fn addresses(&self) -> TransportAddresses {
        TransportAddresses::from(&*self.session)
    }

    fn negotiated_protocol(&self) -> NegotiatedProtocol {
        NegotiatedProtocol::WebTransport {
            protocol: self.session.protocol().map(str::to_owned),
        }
    }
}
