use tracing::Span;

use crate::modules::transport::transport_connection::BoxedConnection;

#[cfg(test)]
use crate::modules::moqt::control_plane::control_messages::messages::client_setup::ClientSetup;

use crate::{
    AuthorizationToken, Session, TerminationErrorCode, TraceContext,
    modules::moqt::{
        domains::{connection::Connection, session_context::SessionContext, session_events},
        version::ServerHandshake,
    },
};

pub struct Handshake {
    pub(crate) transport_connection: BoxedConnection,
    pub(crate) setup: ServerHandshake,
}

impl Handshake {
    /// The AUTHORIZATION TOKEN parameters of the CLIENT_SETUP, for the
    /// application to decide between `accept` and `reject`.
    pub fn authorization_tokens(&self) -> &[AuthorizationToken] {
        self.setup.authorization_tokens()
    }

    /// The traceparent of the client's `moqt.session.connect` span, carried in
    /// CLIENT_SETUP's TRACE_CONTEXT Setup Option.
    pub fn trace_context(&self) -> Option<&TraceContext> {
        self.setup.trace_context()
    }

    #[cfg(test)]
    pub(crate) fn client_setup(&self) -> &ClientSetup {
        let ServerHandshake::Draft14(handshake) = &self.setup else {
            panic!("the CLIENT_SETUP of a draft-14 handshake");
        };
        &handshake.client_setup
    }

    /// Sends SERVER_SETUP in the current span and starts the session's tasks
    /// under `session_span`, so the current span can close once SERVER_SETUP
    /// is sent.
    pub async fn accept(self, session_span: &Span) -> anyhow::Result<Session> {
        let (control, control_receiver) = self.setup.accept(&self.transport_connection).await?;
        let (event_sender, event_receiver) = session_events::channel();
        let context = SessionContext::new(
            Connection::new(
                self.transport_connection,
                control,
                true,
                0,
                event_sender.clone(),
            ),
            event_sender,
            false,
        );
        tracing::debug!("session established");
        Ok(Session::new(
            control_receiver,
            context,
            event_receiver,
            None,
            None,
            session_span.clone(),
        ))
    }

    pub async fn reject(self, code: TerminationErrorCode, reason: &str) {
        self.transport_connection
            .close(self.setup.version().termination_code(code), reason);
        // Wait for the WebTransport CLOSE_SESSION capsule (carrying the code and
        // reason) to be delivered before this connection is dropped; otherwise
        // the QUIC connection is torn down first and the browser only sees a
        // generic "Connection lost." (web-transport-quinn's close() flushes the
        // capsule in a background task).
        self.transport_connection.closed().await;
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        TerminationErrorCode,
        modules::{
            moqt::control_plane::constants::MOQ_TRANSPORT_VERSION,
            test_support::{HANDSHAKE_TIMEOUT, native_client, spawn_native_server_handshake},
        },
    };

    #[tokio::test]
    async fn client_setup_is_exposed_before_server_setup() {
        // Arrange
        let (port, accept) = spawn_native_server_handshake("handshake-client-setup");
        let url = format!("moqt://127.0.0.1:{port}");
        let client = tokio::spawn(async move { native_client().connect(&url).await?.await });

        // Act
        let handshake = tokio::time::timeout(HANDSHAKE_TIMEOUT, accept)
            .await
            .unwrap()
            .unwrap();

        // Assert
        assert!(
            handshake
                .client_setup()
                .supported_versions
                .contains(&MOQ_TRANSPORT_VERSION)
        );
        client.abort();
    }

    #[tokio::test]
    async fn accept_establishes_session() {
        // Arrange
        let (port, accept) = spawn_native_server_handshake("handshake-accept");
        let url = format!("moqt://127.0.0.1:{port}");
        let client = tokio::spawn(async move { native_client().connect(&url).await?.await });
        let handshake = tokio::time::timeout(HANDSHAKE_TIMEOUT, accept)
            .await
            .unwrap()
            .unwrap();

        // Act
        let server = handshake.accept(&tracing::Span::current()).await;
        let client = tokio::time::timeout(HANDSHAKE_TIMEOUT, client)
            .await
            .unwrap()
            .unwrap();

        // Assert
        assert!(server.is_ok());
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn reject_closes_connection_before_server_setup() {
        // Arrange
        let (port, accept) = spawn_native_server_handshake("handshake-reject");
        let url = format!("moqt://127.0.0.1:{port}");
        let client = tokio::spawn(async move { native_client().connect(&url).await?.await });
        let handshake = tokio::time::timeout(HANDSHAKE_TIMEOUT, accept)
            .await
            .unwrap()
            .unwrap();

        // Act
        handshake
            .reject(TerminationErrorCode::Unauthorized, "test")
            .await;
        let client = tokio::time::timeout(HANDSHAKE_TIMEOUT, client)
            .await
            .unwrap()
            .unwrap();

        // Assert
        assert!(client.is_err());
    }
}
