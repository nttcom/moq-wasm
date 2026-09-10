use std::sync::atomic::AtomicU64;

use crate::{
    Session, TerminationErrorCode, TransportProtocol,
    modules::{
        moqt::{
            control_plane::control_messages::messages::client_setup::ClientSetup,
            data_plane::stream::{
                bi_stream_sender::BiStreamSender, stream_receiver::BiStreamReceiver,
            },
            domains::{
                session_context::SessionContext, session_context_factory::SessionContextFactory,
            },
        },
        transport::transport_connection::TransportConnection,
    },
};

pub struct Handshake<T: TransportProtocol> {
    pub(crate) client_setup: ClientSetup,
    pub(crate) transport_connection: T::Connection,
    pub(crate) send_stream: BiStreamSender<T>,
    pub(crate) receive_stream: BiStreamReceiver<T>,
}

impl<T: TransportProtocol> Handshake<T> {
    pub fn client_setup(&self) -> &ClientSetup {
        &self.client_setup
    }

    pub async fn accept(mut self) -> anyhow::Result<Session<T>> {
        SessionContextFactory::send_server_setup(&mut self.send_stream).await?;
        let (event_sender, event_receiver) = tokio::sync::mpsc::unbounded_channel();
        let context = SessionContext::new(
            self.transport_connection,
            self.send_stream,
            AtomicU64::new(1),
            event_sender,
        );
        tracing::info!("Session is established.");
        Ok(Session::new(self.receive_stream, context, event_receiver))
    }

    pub async fn reject(self, code: TerminationErrorCode, reason: &str) {
        self.transport_connection.close(code as u32, reason);
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
            test_support::{HANDSHAKE_TIMEOUT, dual_client, spawn_dual_server_handshake},
        },
    };

    #[tokio::test]
    async fn client_setup_is_exposed_before_server_setup() {
        // Arrange
        let (port, accept) = spawn_dual_server_handshake("handshake-client-setup");
        let url = format!("moqt://127.0.0.1:{port}");
        let client = tokio::spawn(async move { dual_client().connect(&url).await?.await });

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
        let (port, accept) = spawn_dual_server_handshake("handshake-accept");
        let url = format!("moqt://127.0.0.1:{port}");
        let client = tokio::spawn(async move { dual_client().connect(&url).await?.await });
        let handshake = tokio::time::timeout(HANDSHAKE_TIMEOUT, accept)
            .await
            .unwrap()
            .unwrap();

        // Act
        let server = handshake.accept().await;
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
        let (port, accept) = spawn_dual_server_handshake("handshake-reject");
        let url = format!("moqt://127.0.0.1:{port}");
        let client = tokio::spawn(async move { dual_client().connect(&url).await?.await });
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
