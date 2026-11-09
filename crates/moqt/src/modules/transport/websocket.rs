//! QMux over a WebSocket on TCP + TLS (tokio-tungstenite and tokio-rustls):
//! the relay side of the browser fallback and the `wss://` client.

pub(crate) mod tungstenite_transport;
pub(crate) mod websocket_client;
pub(crate) mod websocket_listener;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::sync::{Notify, watch};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::modules::{executor, transport::qmux::session_state::wait_until};

/// The TLS ALPN of the HTTP/1.1 connection a WebSocket upgrades.
pub(crate) const HTTP11_ALPN: &[u8] = b"http/1.1";
/// TCP, TLS, the HTTP upgrade and the QMux transport parameters together.
pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Messages are QMux records of at most `max_record_size` (16382 by
/// default); the WebSocket limit sits above it so that an oversized record
/// is answered with QMux's FRAME_ENCODING_ERROR instead of a socket error.
pub(crate) fn web_socket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(64 * 1024))
        .max_frame_size(Some(64 * 1024))
}

/// The WebSocket sessions an endpoint handed out whose transport has not
/// ended yet, for `wait_idle`: a session counts until its last record and
/// WebSocket Close are written, not merely until it is closed.
#[derive(Default)]
pub(crate) struct LiveConnections {
    count: Mutex<usize>,
    notify: Notify,
}

impl LiveConnections {
    pub(crate) fn track(self: &Arc<Self>, mut transport_ended: watch::Receiver<bool>) {
        *self.count.lock().expect("live connections lock") += 1;
        let live = self.clone();
        executor::spawn("WebSocket Session Watcher", async move {
            let _ = transport_ended.wait_for(|ended| *ended).await;
            *live.count.lock().expect("live connections lock") -= 1;
            live.notify.notify_waiters();
        });
    }

    pub(crate) async fn wait_idle(&self) {
        wait_until(&self.notify, || {
            (*self.count.lock().expect("live connections lock") == 0).then_some(())
        })
        .await;
    }
}
