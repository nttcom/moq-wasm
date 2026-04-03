use async_trait::async_trait;

use super::wt_receive_stream::WtReceiveStream;
use super::wt_send_stream::WtSendStream;
use crate::modules::transport::transport_connection::TransportConnection;

#[derive(Debug)]
pub struct WtConnection {
    session: web_transport_quinn::Session,
}

impl WtConnection {
    pub(crate) fn new(session: web_transport_quinn::Session) -> Self {
        Self { session }
    }
}

#[async_trait]
impl TransportConnection for WtConnection {
    type SendStream = WtSendStream;
    type ReceiveStream = WtReceiveStream;

    async fn open_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)> {
        let (send, recv) = self.session.open_bi().await?;
        let stable_id = self.session.stable_id();
        let stream_id = send.quic_id().index();
        let send_stream = WtSendStream {
            stable_id,
            stream_id,
            send_stream: send,
        };
        let receive_stream = WtReceiveStream {
            stable_id,
            stream_id,
            recv_stream: recv,
        };
        Ok((send_stream, receive_stream))
    }

    async fn accept_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)> {
        let (send, recv) = self.session.accept_bi().await?;
        let stable_id = self.session.stable_id();
        let stream_id = send.quic_id().index();
        let send_stream = WtSendStream {
            stable_id,
            stream_id,
            send_stream: send,
        };
        let receive_stream = WtReceiveStream {
            stable_id,
            stream_id,
            recv_stream: recv,
        };
        Ok((send_stream, receive_stream))
    }

    async fn open_uni(&self) -> anyhow::Result<Self::SendStream> {
        let send = self.session.open_uni().await?;
        let stable_id = self.session.stable_id();
        let stream_id = send.quic_id().index();
        Ok(WtSendStream {
            stable_id,
            stream_id,
            send_stream: send,
        })
    }

    async fn accept_uni(&self) -> anyhow::Result<Self::ReceiveStream> {
        let recv = self.session.accept_uni().await?;
        let stable_id = self.session.stable_id();
        let stream_id = recv.quic_id().index();
        Ok(WtReceiveStream {
            stable_id,
            stream_id,
            recv_stream: recv,
        })
    }

    fn send_datagram(&self, bytes: bytes::BytesMut) -> anyhow::Result<()> {
        Ok(self.session.send_datagram(bytes.freeze())?)
    }

    async fn receive_datagram(&self) -> anyhow::Result<bytes::BytesMut> {
        match self.session.read_datagram().await {
            Ok(datagram) => Ok(bytes::BytesMut::from(&datagram[..])),
            Err(e) => {
                tracing::error!("Failed to receive datagram: {:?}", e);
                anyhow::bail!("Failed to receive datagram: {:?}", e)
            }
        }
    }
}
