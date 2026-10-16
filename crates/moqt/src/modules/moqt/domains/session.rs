use std::sync::Arc;

use anyhow::bail;
use tracing::Span;

use crate::Publisher;
use crate::Subscriber;
use crate::modules::moqt::control_plane::constants::TerminationErrorCode;
use crate::modules::moqt::control_plane::control_messages::messages::go_away::GoAway;
use crate::modules::moqt::control_plane::control_messages::messages::max_request_id::MaxRequestId;
use crate::modules::moqt::control_plane::control_messages::messages::requests_blocked::RequestsBlocked;
use crate::modules::moqt::control_plane::control_messages::messages::server_setup::ServerSetup;
use crate::modules::moqt::control_plane::enums::SessionEvent;
use crate::modules::moqt::domains::session_context::SessionContext;
use crate::modules::moqt::runtime::tasks::connection_tasks::ConnectionTasks;
use crate::modules::moqt::version::{ControlReceiver, Notification};
use crate::modules::transport::{
    application_protocol::NegotiatedProtocol, transport_addresses::TransportAddresses,
    transport_connection::TransportClose, transport_stats::TransportStats,
};

pub struct Session {
    inner: Arc<SessionContext>,
    session_span: Span,
    event_receiver: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<SessionEvent>>,
    _tasks: ConnectionTasks,
    server_setup: Option<ServerSetup>,
}

impl Session {
    // On wasm32 the browser transport is !Send, but the context is shared with
    // the native build, where the tasks run on a multi-threaded executor.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub(crate) fn new(
        control_receiver: ControlReceiver,
        inner: SessionContext,
        event_receiver: tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
        server_setup: Option<ServerSetup>,
    ) -> Self {
        let inner = Arc::new(inner);
        let parent_span = Span::current();
        let session_span = tracing::info_span!(parent: &parent_span, "moqt.session");
        let tasks =
            ConnectionTasks::run(&inner, inner.connection(), control_receiver, &session_span);
        Self {
            inner,
            session_span,
            event_receiver: tokio::sync::Mutex::new(event_receiver),
            _tasks: tasks,
            server_setup,
        }
    }

    pub(crate) fn context(&self) -> &Arc<SessionContext> {
        &self.inner
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

    /// Resolves once the transport has closed, whichever side closed it.
    /// Consumes no session events, so it can be awaited beside
    /// `receive_event()`.
    pub async fn closed(&self) -> TransportClose {
        self.inner.connection().transport.closed().await
    }

    pub fn transport_stats(&self) -> TransportStats {
        self.inner.connection().transport.stats()
    }

    pub fn transport_addresses(&self) -> TransportAddresses {
        self.inner.connection().transport.addresses()
    }

    /// The URL scheme `Endpoint::connect` takes for this session's transport:
    /// `moqt` for raw QUIC, `https` for WebTransport. A server names a GOAWAY's
    /// New Session URI in it (draft-14 §9.4).
    pub fn url_scheme(&self) -> &'static str {
        match self.inner.connection().transport.negotiated_protocol() {
            NegotiatedProtocol::Quic { .. } => "moqt",
            NegotiatedProtocol::WebTransport { .. } => "https",
        }
    }

    /// draft-14 §9.4. A client sends an empty URI; only a server names a new
    /// session. Fire-and-forget: the draft defines no response.
    pub async fn go_away(&self, new_session_uri: String) -> anyhow::Result<()> {
        self.inner
            .connection()
            .control
            .notify(Notification::GoAway(GoAway::new(new_session_uri)))
            .await?;
        Ok(())
    }

    /// Allows the peer to use request ids below `max_request_id` (draft-14
    /// §9.2). Fire-and-forget.
    pub async fn raise_max_request_id(&self, max_request_id: u64) -> anyhow::Result<()> {
        self.inner
            .connection()
            .control
            .notify(Notification::MaxRequestId(MaxRequestId::new(
                max_request_id,
            )))
            .await?;
        Ok(())
    }

    /// Tells the peer this side ran out of request ids at
    /// `maximum_request_id` (draft-14 §9.3). Fire-and-forget.
    pub async fn requests_blocked(&self, maximum_request_id: u64) -> anyhow::Result<()> {
        self.inner
            .connection()
            .control
            .notify(Notification::RequestsBlocked(RequestsBlocked::new(
                maximum_request_id,
            )))
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
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        SessionEvent, TerminationErrorCode,
        modules::test_support::{
            DISCONNECT_TIMEOUT, connect_sessions, receive_disconnected,
            spawn_connected_dual_sessions, spawn_dual_server,
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
    async fn url_scheme_is_the_one_the_client_connected_with() {
        // Arrange
        let (quic_port, quic_accept) = spawn_dual_server("url-scheme-quic");
        let (wt_port, wt_accept) = spawn_dual_server("url-scheme-wt");

        // Act
        let (quic_client, quic_server) =
            connect_sessions(&format!("moqt://127.0.0.1:{quic_port}"), quic_accept)
                .await
                .unwrap();
        let (wt_client, wt_server) =
            connect_sessions(&format!("https://127.0.0.1:{wt_port}/moq"), wt_accept)
                .await
                .unwrap();

        // Assert
        assert_eq!(quic_client.url_scheme(), "moqt");
        assert_eq!(quic_server.url_scheme(), "moqt");
        assert_eq!(wt_client.url_scheme(), "https");
        assert_eq!(wt_server.url_scheme(), "https");
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
    async fn closed_reports_the_peers_termination_code_and_leaves_the_events_unread() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("session-closed").await;

        // Act
        server.close_with_error(TerminationErrorCode::GoawayTimeout, "bye");
        let close = tokio::time::timeout(DISCONNECT_TIMEOUT, client.closed())
            .await
            .unwrap();

        // Assert: draft-14 numbers GOAWAY_TIMEOUT 0x10
        assert_eq!(close.code, Some(0x10));
        assert_eq!(close.reason, "bye");
        receive_disconnected(&client).await.unwrap();
    }

    #[tokio::test]
    async fn server_go_away_names_the_new_session_uri_to_the_client() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("session-go-away-uri").await;

        // Act
        server
            .go_away("moqt://relay-b.example:4433".to_string())
            .await
            .unwrap();

        // Assert
        let SessionEvent::GoAway(go_away) = client.receive_event().await.unwrap() else {
            panic!("expected GOAWAY from the server");
        };
        assert_eq!(go_away.new_session_uri(), "moqt://relay-b.example:4433");
    }

    #[tokio::test]
    async fn second_go_away_is_a_protocol_violation() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("session-go-away-twice").await;
        server.go_away(String::new()).await.unwrap();
        assert!(matches!(
            client.receive_event().await.unwrap(),
            SessionEvent::GoAway(_)
        ));

        // Act
        server.go_away(String::new()).await.unwrap();

        // Assert: the client closes the session
        let close = tokio::time::timeout(DISCONNECT_TIMEOUT, server.closed())
            .await
            .unwrap();
        assert_eq!(close.code, Some(0x3));
        assert_eq!(close.reason, "received a second GOAWAY");
    }

    #[tokio::test]
    async fn go_away_with_a_uri_sent_to_a_server_is_a_protocol_violation() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("session-go-away-to-server").await;

        // Act
        client
            .go_away("moqt://relay-b.example:4433".to_string())
            .await
            .unwrap();

        // Assert: the server closes the session
        let close = tokio::time::timeout(DISCONNECT_TIMEOUT, client.closed())
            .await
            .unwrap();
        assert_eq!(close.code, Some(0x3));
        assert_eq!(
            close.reason,
            "GOAWAY sent to a server carries a New Session URI"
        );
        drop(server);
    }

    #[tokio::test]
    async fn closing_with_a_non_violation_code_reports_disconnected_locally() {
        // Arrange
        let (_client, server) = spawn_connected_dual_sessions("session-local-close").await;

        // Act
        server.close_with_error(TerminationErrorCode::GoawayTimeout, "drain timed out");

        // Assert
        let event = tokio::time::timeout(DISCONNECT_TIMEOUT, server.receive_event())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event, SessionEvent::Disconnected()), "{event:?}");
    }

    #[tokio::test]
    async fn closing_for_a_protocol_violation_reports_it_locally() {
        // Arrange
        let (_client, server) = spawn_connected_dual_sessions("session-local-violation").await;

        // Act
        server.close_with_error(TerminationErrorCode::ProtocolViolation, "bad peer");

        // Assert
        let event = tokio::time::timeout(DISCONNECT_TIMEOUT, server.receive_event())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(event, SessionEvent::ProtocolViolation()),
            "{event:?}"
        );
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
