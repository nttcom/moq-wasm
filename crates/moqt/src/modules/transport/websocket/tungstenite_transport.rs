use async_trait::async_trait;
use bytes::Bytes;
use futures::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Error, Message,
        protocol::{CloseFrame, frame::coding::CloseCode},
    },
};

use crate::modules::transport::qmux::message_transport::{MessageSink, MessageSource};

pub(crate) struct TungsteniteSink<S> {
    sink: SplitSink<WebSocketStream<S>, Message>,
}

pub(crate) struct TungsteniteSource<S> {
    stream: SplitStream<WebSocketStream<S>>,
}

pub(crate) fn split<S>(stream: WebSocketStream<S>) -> (TungsteniteSink<S>, TungsteniteSource<S>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (sink, stream) = stream.split();
    (TungsteniteSink { sink }, TungsteniteSource { stream })
}

#[async_trait]
impl<S> MessageSink for TungsteniteSink<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    async fn send(&mut self, message: Bytes) -> anyhow::Result<()> {
        Ok(self.sink.send(Message::Binary(message)).await?)
    }

    async fn close(&mut self, reason: &str) -> anyhow::Result<()> {
        let frame = CloseFrame {
            code: CloseCode::Normal,
            reason: reason.to_string().into(),
        };
        match self.sink.send(Message::Close(Some(frame))).await {
            Ok(()) | Err(Error::ConnectionClosed | Error::AlreadyClosed) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

#[async_trait]
impl<S> MessageSource for TungsteniteSource<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    async fn receive(&mut self) -> anyhow::Result<Option<Bytes>> {
        loop {
            return match self.stream.next().await {
                None | Some(Ok(Message::Close(_))) => Ok(None),
                Some(Err(Error::ConnectionClosed | Error::AlreadyClosed)) => Ok(None),
                Some(Ok(Message::Binary(message))) => Ok(Some(message)),
                // tungstenite answers Ping itself; neither carries QMux frames.
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
                // draft-lcurley-qmux-websocket-00 §7.
                Some(Ok(Message::Text(_))) => {
                    anyhow::bail!("received a WebSocket text message")
                }
                Some(Err(error)) => Err(error.into()),
            };
        }
    }
}
