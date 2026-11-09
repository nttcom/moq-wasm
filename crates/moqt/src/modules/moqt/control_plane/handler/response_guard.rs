use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::{
    TrackProperties,
    modules::{
        executor,
        moqt::{
            control_plane::error_codes::RequestErrorCode,
            domains::connection::Connection,
            version::{OutgoingResponse, RequestSummary},
        },
        transport::transport_send_stream::TransportSendError,
    },
};

/// Sends and logs the reply of one request, and auto-answers NOT_SUPPORTED
/// when the last handler clone is dropped without responding. An application
/// that ignores the session event would otherwise leave the requester waiting
/// for its control timeout.
#[derive(Debug, Clone)]
pub(crate) struct ResponseGuard {
    /// The connection the request arrived on; every answer goes there.
    connection: Arc<Connection>,
    request: RequestSummary,
    responded: Arc<AtomicBool>,
}

impl ResponseGuard {
    pub(crate) fn new(connection: Arc<Connection>, request: RequestSummary) -> Self {
        Self {
            connection,
            request,
            responded: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn connection(&self) -> &Arc<Connection> {
        &self.connection
    }

    /// The request's id on its connection.
    pub(crate) fn request_id(&self) -> u64 {
        self.request.request_id()
    }

    pub(crate) fn mark_responded(&self) {
        self.responded.store(true, Ordering::Relaxed);
    }

    /// Refuses the request with UNSUPPORTED_EXTENSION, NOT_SUPPORTED on
    /// draft-14, and fails when the peer's draft does not understand
    /// `properties` (draft-18 §2.5.1).
    pub(crate) async fn refuse_unless_understood(
        &self,
        properties: &TrackProperties,
    ) -> anyhow::Result<()> {
        if self.connection.control.version().understands(properties) {
            return Ok(());
        }
        self.reject(
            RequestErrorCode::UnsupportedExtension,
            "mandatory track property not understood".to_string(),
        )
        .await?;
        anyhow::bail!("the peer's draft cannot carry the track's properties")
    }

    /// `response` must be an `*_OK`; it is logged as `*_OK sent`.
    pub(crate) async fn accept(
        &self,
        response: OutgoingResponse,
    ) -> Result<(), TransportSendError> {
        self.mark_responded();
        self.connection.control.respond(response).await?;
        self.request.log_ok_sent();
        Ok(())
    }

    pub(crate) async fn reject(
        &self,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), TransportSendError> {
        self.mark_responded();
        send_request_error(&self.connection, &self.request, code, reason_phrase).await
    }
}

async fn send_request_error(
    connection: &Connection,
    request: &RequestSummary,
    code: RequestErrorCode,
    reason_phrase: String,
) -> Result<(), TransportSendError> {
    connection
        .control
        .respond(OutgoingResponse::Error {
            kind: request.kind(),
            request_id: request.request_id(),
            code,
            reason_phrase: reason_phrase.clone(),
        })
        .await?;
    request.log_error_sent(code, &reason_phrase);
    Ok(())
}

impl Drop for ResponseGuard {
    fn drop(&mut self) {
        // strong_count == 1 limits this to the last clone; a single
        // fire-and-forget send needs no owned JoinHandle.
        if self.responded.load(Ordering::Relaxed) || Arc::strong_count(&self.responded) > 1 {
            return;
        }
        let connection = self.connection.clone();
        let request = self.request.clone();
        executor::try_spawn("Unhandled Request Rejecter", async move {
            if let Err(error) = send_request_error(
                &connection,
                &request,
                RequestErrorCode::NotSupported,
                "request not handled by application".to_string(),
            )
            .await
            {
                tracing::warn!(
                    ?error,
                    request_id = request.request_id(),
                    kind = ?request.kind(),
                    "failed to auto-reject unhandled request"
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        RequestErrorCode, SessionEvent,
        modules::test_support::{capture_telemetry, spawn_connected_native_sessions},
    };

    #[tokio::test]
    async fn ok_is_logged_with_the_request_id_and_track_namespace_the_peer_sent() {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();
        let (client, server) = spawn_connected_native_sessions("response-guard-ok-log").await;
        let publisher = client.publisher();
        let request =
            tokio::spawn(async move { publisher.publish_namespace("a/b".to_string()).await });
        let SessionEvent::PublishNamespace(handler) = server.receive_event().await.unwrap() else {
            panic!("expected PUBLISH_NAMESPACE from the client");
        };

        // Act
        handler.ok().await.unwrap();
        request.await.unwrap().unwrap();

        // Assert
        let sent = telemetry.events("PUBLISH_NAMESPACE_OK sent");
        let received = telemetry.events("PUBLISH_NAMESPACE_OK received");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].field("track_namespace"), Some("a/b"));
        assert_eq!(sent[0].field("request_id"), received[0].field("request_id"));
    }

    #[tokio::test]
    async fn error_is_logged_with_its_code_and_reason() {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();
        let (client, server) = spawn_connected_native_sessions("response-guard-error-log").await;
        let publisher = client.publisher();
        let request =
            tokio::spawn(async move { publisher.publish_namespace("a/b".to_string()).await });
        let SessionEvent::PublishNamespace(handler) = server.receive_event().await.unwrap() else {
            panic!("expected PUBLISH_NAMESPACE from the client");
        };

        // Act
        handler
            .error(RequestErrorCode::Unauthorized, "denied".to_string())
            .await
            .unwrap();
        let _ = request.await.unwrap();

        // Assert
        let sent = telemetry.events("PUBLISH_NAMESPACE_ERROR sent");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].field("track_namespace"), Some("a/b"));
        assert_eq!(sent[0].field("error_code"), Some("Unauthorized"));
        assert_eq!(sent[0].field("reason_phrase"), Some("denied"));
    }

    #[tokio::test]
    async fn auto_reject_of_a_dropped_handler_is_logged() {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();
        let (client, server) =
            spawn_connected_native_sessions("response-guard-auto-reject-log").await;
        let publisher = client.publisher();
        let request =
            tokio::spawn(async move { publisher.publish_namespace("a/b".to_string()).await });
        let SessionEvent::PublishNamespace(handler) = server.receive_event().await.unwrap() else {
            panic!("expected PUBLISH_NAMESPACE from the client");
        };

        // Act
        drop(handler);
        let _ = request.await.unwrap();

        // Assert
        let sent = telemetry.events("PUBLISH_NAMESPACE_ERROR sent");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].field("error_code"), Some("NotSupported"));
    }
}
