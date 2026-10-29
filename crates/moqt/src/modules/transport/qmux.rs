//! QMux (draft-ietf-quic-qmux-02): the QUIC version 1 stream and datagram
//! operations over an ordered, reliable message transport. Here the transport
//! is a WebSocket (draft-lcurley-qmux-websocket-00): every binary message is
//! the `Frames` field of one QMux record, with no `Size` prefix.

pub(crate) mod frame;
#[cfg(test)]
pub(crate) mod in_memory;
pub(crate) mod message_transport;
pub(crate) mod qmux_connection;
pub(crate) mod qmux_receive_stream;
pub(crate) mod qmux_send_stream;
pub(crate) mod session_state;
pub(crate) mod stats;
pub(crate) mod stream_id;
pub(crate) mod subprotocol;
pub(crate) mod tasks;
pub(crate) mod transport_parameters;
