//! QMux over the browser's `WebSocket` (draft-lcurley-qmux-websocket-00).

use std::{rc::Rc, time::Duration};

use anyhow::Context;
use async_trait::async_trait;
use bytes::Bytes;
use js_sys::{ArrayBuffer, Uint8Array};
use tokio::sync::mpsc;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{BinaryType, CloseEvent, Event, MessageEvent, WebSocket};

use super::js_value::js_error;
use crate::modules::{
    executor,
    transport::{
        application_protocol::ApplicationProtocols,
        connect_target::ConnectTarget,
        qmux::{
            message_transport::{MessageSink, MessageSource},
            qmux_connection::QmuxConnection,
            stream_id::Role,
            subprotocol,
        },
        transport_addresses::TransportAddresses,
    },
};

/// What the browser has queued on the socket before `send` waits for it to
/// drain, so that a slow TCP path pushes back on the QMux send task.
const SEND_BUFFER_LIMIT: u32 = 1 << 20;
const SEND_BUFFER_POLL: Duration = Duration::from_millis(5);
/// RFC 6455 §5.5: a Close frame's payload is at most 125 bytes, 2 of them the code.
const MAX_CLOSE_REASON_BYTES: usize = 123;
const NORMAL_CLOSURE: u16 = 1000;

enum SocketEvent {
    Open,
    Message(Bytes),
    TextMessage,
    Closed { code: u16, reason: String },
    Error,
}

/// The socket and the handlers that feed its events into a channel; the
/// handlers are removed and the socket closed once the sink and the source
/// are both gone.
struct Socket {
    web_socket: WebSocket,
    _on_open: Closure<dyn FnMut(Event)>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_close: Closure<dyn FnMut(CloseEvent)>,
    _on_error: Closure<dyn FnMut(Event)>,
}

impl Socket {
    fn open(
        url: &str,
        protocols: &[String],
    ) -> anyhow::Result<(Rc<Self>, mpsc::UnboundedReceiver<SocketEvent>)> {
        let offered: js_sys::Array = protocols
            .iter()
            .map(|protocol| JsValue::from_str(protocol))
            .collect();
        let web_socket = WebSocket::new_with_str_sequence(url, &offered).map_err(js_error)?;
        web_socket.set_binary_type(BinaryType::Arraybuffer);
        let (event_sender, event_receiver) = mpsc::unbounded_channel();

        let sender = event_sender.clone();
        let on_open = Closure::<dyn FnMut(Event)>::new(move |_: Event| {
            let _ = sender.send(SocketEvent::Open);
        });
        let sender = event_sender.clone();
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            let message = match event.data().dyn_into::<ArrayBuffer>() {
                Ok(buffer) => SocketEvent::Message(Bytes::from(Uint8Array::new(&buffer).to_vec())),
                Err(_) => SocketEvent::TextMessage,
            };
            let _ = sender.send(message);
        });
        let sender = event_sender.clone();
        let on_close = Closure::<dyn FnMut(CloseEvent)>::new(move |event: CloseEvent| {
            let _ = sender.send(SocketEvent::Closed {
                code: event.code(),
                reason: event.reason(),
            });
        });
        let sender = event_sender;
        let on_error = Closure::<dyn FnMut(Event)>::new(move |_: Event| {
            let _ = sender.send(SocketEvent::Error);
        });
        web_socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        web_socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        web_socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        web_socket.set_onerror(Some(on_error.as_ref().unchecked_ref()));

        let socket = Rc::new(Self {
            web_socket,
            _on_open: on_open,
            _on_message: on_message,
            _on_close: on_close,
            _on_error: on_error,
        });
        Ok((socket, event_receiver))
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.web_socket.set_onopen(None);
        self.web_socket.set_onmessage(None);
        self.web_socket.set_onclose(None);
        self.web_socket.set_onerror(None);
        if self.web_socket.ready_state() <= WebSocket::OPEN {
            let _ = self.web_socket.close_with_code(NORMAL_CLOSURE);
        }
    }
}

struct BrowserWebSocketSink {
    socket: Rc<Socket>,
}

struct BrowserWebSocketSource {
    _socket: Rc<Socket>,
    events: mpsc::UnboundedReceiver<SocketEvent>,
}

#[async_trait(?Send)]
impl MessageSink for BrowserWebSocketSink {
    async fn send(&mut self, message: Bytes) -> anyhow::Result<()> {
        let web_socket = &self.socket.web_socket;
        while web_socket.buffered_amount() > SEND_BUFFER_LIMIT {
            if web_socket.ready_state() != WebSocket::OPEN {
                anyhow::bail!("WebSocket closed");
            }
            executor::sleep(SEND_BUFFER_POLL).await;
        }
        anyhow::ensure!(
            web_socket.ready_state() == WebSocket::OPEN,
            "WebSocket closed"
        );
        web_socket.send_with_u8_array(&message).map_err(js_error)
    }

    async fn close(&mut self, reason: &str) -> anyhow::Result<()> {
        let mut end = reason.len().min(MAX_CLOSE_REASON_BYTES);
        while !reason.is_char_boundary(end) {
            end -= 1;
        }
        self.socket
            .web_socket
            .close_with_code_and_reason(NORMAL_CLOSURE, &reason[..end])
            .map_err(js_error)
    }
}

#[async_trait(?Send)]
impl MessageSource for BrowserWebSocketSource {
    async fn receive(&mut self) -> anyhow::Result<Option<Bytes>> {
        match self.events.recv().await {
            Some(SocketEvent::Message(message)) => Ok(Some(message)),
            // draft-lcurley-qmux-websocket-00 §7.
            Some(SocketEvent::TextMessage) => anyhow::bail!("received a WebSocket text message"),
            Some(SocketEvent::Closed { .. }) | None => Ok(None),
            Some(SocketEvent::Error) => anyhow::bail!("WebSocket error"),
            Some(SocketEvent::Open) => unreachable!("connect consumes the open event"),
        }
    }
}

/// Opens a QMux session over `wss://`. Fails when the handshake fails or the
/// server selects no subprotocol this side offered (§5).
pub(crate) async fn connect(
    target: &ConnectTarget,
    protocols: ApplicationProtocols,
) -> anyhow::Result<QmuxConnection> {
    let offered = protocols.web_socket_subprotocols();
    let (socket, mut events) = Socket::open(&target.url, &offered)?;
    match events.recv().await {
        Some(SocketEvent::Open) => {}
        Some(SocketEvent::Closed { code, reason }) => {
            anyhow::bail!("WebSocket handshake failed: close {code} {reason}")
        }
        _ => anyhow::bail!("WebSocket handshake failed"),
    }
    let selected = socket.web_socket.protocol();
    let selected = offered
        .iter()
        .find(|offer| **offer == selected)
        .context("the server selected no offered WebSocket subprotocol")?;
    let alpn = subprotocol::alpn_of(selected).flatten().map(str::to_string);
    tracing::debug!(%selected, "WebSocket upgraded");
    QmuxConnection::establish(
        BrowserWebSocketSink {
            socket: socket.clone(),
        },
        BrowserWebSocketSource {
            _socket: socket,
            events,
        },
        Role::Client,
        alpn,
        TransportAddresses::default(),
    )
    .await
}
