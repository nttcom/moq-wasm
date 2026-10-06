use std::sync::Arc;

use anyhow::bail;
use tracing::Span;

use crate::Publisher;
use crate::Subscriber;
use crate::modules::executor::JoinHandle;
use crate::modules::moqt::control_plane::constants::TerminationErrorCode;
use crate::modules::moqt::control_plane::control_messages::control_message_type::ControlMessageType;
use crate::modules::moqt::control_plane::control_messages::messages::go_away::GoAway;
use crate::modules::moqt::control_plane::control_messages::messages::max_request_id::MaxRequestId;
use crate::modules::moqt::control_plane::control_messages::messages::requests_blocked::RequestsBlocked;
use crate::modules::moqt::control_plane::control_messages::messages::server_setup::ServerSetup;
use crate::modules::moqt::control_plane::enums::SessionEvent;
use crate::modules::moqt::data_plane::stream::stream_receiver::BiStreamReceiver;
use crate::modules::moqt::domains::session_context::SessionContext;
use crate::modules::moqt::runtime::tasks::{
    control_message_receive_task::ControlMessageReceiveTask,
    datagram_receive_task::DatagramReceiveTask, disconnect_watch_task::DisconnectWatchTask,
    uni_stream_receive_task::UniStreamReceiveTask,
};
use crate::modules::transport::{
    transport_addresses::TransportAddresses, transport_stats::TransportStats,
};

pub struct Session {
    inner: Arc<SessionContext>,
    session_span: Span,
    event_receiver: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<SessionEvent>>,
    control_message_receive_task: JoinHandle,
    datagram_receive_task: JoinHandle,
    uni_stream_receive_task: JoinHandle,
    disconnect_watch_task: JoinHandle,
    server_setup: Option<ServerSetup>,
}

impl Session {
    // On wasm32 the browser transport is !Send, but the context is shared with
    // the native build, where the tasks run on a multi-threaded executor.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub(crate) fn new(
        receive_stream: BiStreamReceiver,
        inner: SessionContext,
        event_receiver: tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
        server_setup: Option<ServerSetup>,
    ) -> Self {
        let inner = Arc::new(inner);
        let parent_span = Span::current();
        let session_span = tracing::info_span!(parent: &parent_span, "moqt.session");
        let control_plane_receiver_span = tracing::info_span!(
            parent: &session_span,
            "moqt.control_plane.receiver"
        );
        let datagram_receiver_span =
            tracing::info_span!(parent: &session_span, "data_plane.datagram_receiver");
        let uni_stream_receiver_span =
            tracing::info_span!(parent: &session_span, "data_plane.uni_stream_receiver");
        let transport_close_watcher_span =
            tracing::info_span!(parent: &session_span, "moqt.transport.close_watcher");

        let control_message_receive_task = ControlMessageReceiveTask::run(
            receive_stream,
            Arc::downgrade(&inner),
            control_plane_receiver_span,
        );
        let datagram_receive_task = DatagramReceiveTask::run(inner.clone(), datagram_receiver_span);
        let uni_stream_receive_task =
            UniStreamReceiveTask::run(inner.clone(), uni_stream_receiver_span);
        let disconnect_watch_task =
            DisconnectWatchTask::run(inner.clone(), transport_close_watcher_span);

        Self {
            inner,
            session_span,
            event_receiver: tokio::sync::Mutex::new(event_receiver),
            control_message_receive_task,
            datagram_receive_task,
            uni_stream_receive_task,
            disconnect_watch_task,
            server_setup,
        }
    }

    pub fn publisher(&self) -> Publisher {
        Publisher {
            session: self.inner.clone(),
        }
    }

    pub fn subscriber(&self) -> Subscriber {
        Subscriber {
            session: self.inner.clone(),
        }
    }

    pub fn publisher_subscriber_pair(&self) -> (Publisher, Subscriber) {
        (self.publisher(), self.subscriber())
    }

    /// Terminates the session (draft-14 §3.4): the peer receives `code` and
    /// `reason` on the transport close, and this side observes
    /// `SessionEvent::ProtocolViolation`.
    pub fn close_with_error(&self, code: TerminationErrorCode, reason: &str) {
        self.inner.close_with_error(code, reason);
    }

    pub async fn receive_event(&self) -> anyhow::Result<SessionEvent> {
        match self.event_receiver.lock().await.recv().await {
            Some(v) => Ok(v),
            None => bail!("Sender dropped."),
        }
    }

    pub fn transport_stats(&self) -> TransportStats {
        self.inner.transport_connection.stats()
    }

    pub fn transport_addresses(&self) -> TransportAddresses {
        self.inner.transport_connection.addresses()
    }

    /// draft-14 §9.4. A client sends an empty URI; only a server names a new
    /// session. Fire-and-forget: the draft defines no response.
    pub async fn go_away(&self, new_session_uri: String) -> anyhow::Result<()> {
        self.inner
            .send_stream
            .send(
                ControlMessageType::GoAway,
                GoAway::new(new_session_uri).encode(),
            )
            .await?;
        Ok(())
    }

    /// Allows the peer to use request ids below `max_request_id` (draft-14
    /// §9.2). Fire-and-forget.
    pub async fn raise_max_request_id(&self, max_request_id: u64) -> anyhow::Result<()> {
        self.inner
            .send_stream
            .send(
                ControlMessageType::MaxRequestId,
                MaxRequestId::new(max_request_id).encode(),
            )
            .await?;
        Ok(())
    }

    /// Tells the peer this side ran out of request ids at
    /// `maximum_request_id` (draft-14 §9.3). Fire-and-forget.
    pub async fn requests_blocked(&self, maximum_request_id: u64) -> anyhow::Result<()> {
        self.inner
            .send_stream
            .send(
                ControlMessageType::RequestsBlocked,
                RequestsBlocked::new(maximum_request_id).encode(),
            )
            .await?;
        Ok(())
    }

    /// The SERVER_SETUP this client received; `None` on a server session.
    pub fn server_setup(&self) -> Option<&ServerSetup> {
        self.server_setup.as_ref()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.session_span.in_scope(|| {
            tracing::info!("Session dropped.");
        });
        self.control_message_receive_task.abort();
        self.datagram_receive_task.abort();
        self.uni_stream_receive_task.abort();
        self.disconnect_watch_task.abort();
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        SessionEvent,
        modules::test_support::{
            connect_sessions, receive_disconnected, spawn_connected_dual_sessions,
            spawn_dual_server,
        },
        wire::MOQ_TRANSPORT_VERSION,
    };

    #[tokio::test]
    async fn transport_stats_reports_the_established_quic_path() {
        // Arrange
        let (port, accept) = spawn_dual_server("transport-stats");
        let (client, _server) = connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept)
            .await
            .unwrap();

        // Act
        let stats = client.transport_stats();

        // Assert
        assert!(stats.cwnd > 0);
        assert!(!stats.rtt.is_zero());
        assert_eq!(stats.lost_packets, 0);
        assert!(stats.current_mtu > 0);
        assert!(stats.sent_bytes > 0);
        assert!(stats.received_bytes > 0);
    }

    #[tokio::test]
    async fn transport_addresses_name_the_peer_of_the_quic_path() {
        // Arrange
        let (port, accept) = spawn_dual_server("transport-addresses");
        let (client, _server) = connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept)
            .await
            .unwrap();

        // Act
        let addresses = client.transport_addresses();

        // Assert
        let remote = addresses.remote.expect("a QUIC session knows its peer");
        assert_eq!(remote.port(), port);
        assert!(remote.ip().is_loopback());
    }

    #[tokio::test]
    async fn client_session_keeps_the_server_setup_it_received() {
        // Arrange
        let (port, accept) = spawn_dual_server("session-server-setup");

        // Act
        let (client, server) = connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept)
            .await
            .unwrap();

        // Assert
        assert_eq!(
            client
                .server_setup()
                .map(|server_setup| server_setup.selected_version),
            Some(MOQ_TRANSPORT_VERSION)
        );
        assert!(server.server_setup().is_none());
    }

    #[tokio::test]
    async fn dropping_the_client_session_disconnects_the_server_session() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("session-drop").await;

        // Act
        drop(client);

        // Assert
        receive_disconnected(&server).await.unwrap();
    }

    #[tokio::test]
    async fn go_away_reaches_the_peer_with_its_uri() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("session-go-away").await;

        // Act
        client.go_away(String::new()).await.unwrap();

        // Assert
        let SessionEvent::GoAway(go_away) = server.receive_event().await.unwrap() else {
            panic!("expected GOAWAY from the client");
        };
        assert_eq!(go_away.new_session_uri(), "");
    }

    #[tokio::test]
    async fn raise_max_request_id_reaches_the_peer() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("session-max-request-id").await;

        // Act
        client.raise_max_request_id(200).await.unwrap();

        // Assert
        let SessionEvent::MaxRequestId(max_request_id) = server.receive_event().await.unwrap()
        else {
            panic!("expected MAX_REQUEST_ID from the client");
        };
        assert_eq!(max_request_id.request_id(), 200);
    }

    #[tokio::test]
    async fn requests_blocked_reaches_the_peer() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("session-requests-blocked").await;

        // Act
        client.requests_blocked(100).await.unwrap();

        // Assert
        let SessionEvent::RequestsBlocked(blocked) = server.receive_event().await.unwrap() else {
            panic!("expected REQUESTS_BLOCKED from the client");
        };
        assert_eq!(blocked.maximum_request_id(), 100);
    }
}
