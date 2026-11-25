use std::sync::{Arc, Mutex};

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

/// Whether a request has been answered, shared by all clones of its
/// `ResponseGuard`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Unanswered,
    /// `accept`, `reject` or a session close answered the request.
    Answered,
}

/// Sends and logs the reply of one request, and auto-answers NOT_SUPPORTED
/// when the last handler clone is dropped without responding. An application
/// that ignores the session event would otherwise leave the requester waiting
/// for its control timeout.
#[derive(Debug, Clone)]
pub(crate) struct ResponseGuard {
    inner: Arc<Request>,
}

/// The state every clone of a `ResponseGuard` shares; its `Drop` runs once,
/// when the last clone goes.
#[derive(Debug)]
struct Request {
    /// The connection the request arrived on; every answer goes there.
    connection: Arc<Connection>,
    summary: RequestSummary,
    answer: Mutex<Answer>,
}

impl ResponseGuard {
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub(crate) fn new(connection: Arc<Connection>, request: RequestSummary) -> Self {
        Self {
            inner: Arc::new(Request {
                connection,
                summary: request,
                answer: Mutex::new(Answer::Unanswered),
            }),
        }
    }

    pub(crate) fn connection(&self) -> &Arc<Connection> {
        &self.inner.connection
    }

    /// The request's id on its connection.
    pub(crate) fn request_id(&self) -> u64 {
        self.inner.summary.request_id()
    }

    /// Records that the request is answered, and returns whether this call
    /// did; only the first answer of a request may send a response.
    pub(crate) fn mark_responded(&self) -> bool {
        let mut answer = self.inner.answer.lock().expect("answer lock");
        if *answer != Answer::Unanswered {
            tracing::debug!(
                request_id = self.request_id(),
                "request already answered; response not sent"
            );
            return false;
        }
        *answer = Answer::Answered;
        true
    }

    /// Refuses the request with UNSUPPORTED_EXTENSION, NOT_SUPPORTED on
    /// draft-14, and fails when the peer's draft does not understand
    /// `properties` (draft-18 §2.5.1).
    pub(crate) async fn refuse_unless_understood(
        &self,
        properties: &TrackProperties,
    ) -> anyhow::Result<()> {
        if self.connection().control.version().understands(properties) {
            return Ok(());
        }
        self.reject(
            RequestErrorCode::UnsupportedExtension,
            "mandatory track property not understood".to_string(),
        )
        .await?;
        anyhow::bail!("the peer's draft cannot carry the track's properties")
    }

    /// `accept` and `reject` answer at most once: once the request is
    /// answered, they send nothing and return `Ok(())`.
    ///
    /// `response` must be an `*_OK`; it is logged as `*_OK sent`. An `*_OK`
    /// the peer's draft cannot carry is answered with INTERNAL_ERROR instead,
    /// so the requester does not wait for its timeout, and the returned
    /// `Unencodable` tells the caller the request was refused.
    pub(crate) async fn accept(
        &self,
        response: OutgoingResponse,
    ) -> Result<(), TransportSendError> {
        if !self.mark_responded() {
            return Ok(());
        }
        match self.inner.connection.control.respond(response).await {
            Ok(()) => {
                self.inner.summary.log_ok_sent();
                Ok(())
            }
            Err(unencodable @ TransportSendError::Unencodable { .. }) => {
                send_request_error(
                    &self.inner.connection,
                    &self.inner.summary,
                    RequestErrorCode::InternalError,
                    "response cannot be encoded".to_string(),
                )
                .await?;
                Err(unencodable)
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn reject(
        &self,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), TransportSendError> {
        if !self.mark_responded() {
            return Ok(());
        }
        send_request_error(
            &self.inner.connection,
            &self.inner.summary,
            code,
            reason_phrase,
        )
        .await
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

impl Drop for Request {
    fn drop(&mut self) {
        if *self.answer.get_mut().expect("answer lock") != Answer::Unanswered {
            return;
        }
        let connection = self.connection.clone();
        let request = self.summary.clone();
        // A single fire-and-forget send needs no owned JoinHandle.
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
        modules::test_support::{capture_telemetry, next_event, spawn_connected_native_sessions},
    };

    #[tokio::test]
    async fn ok_is_logged_with_the_request_id_and_track_namespace_the_peer_sent() {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();
        let (client, server) = spawn_connected_native_sessions("response-guard-ok-log").await;
        let publisher = client.publisher();
        let request =
            tokio::spawn(async move { publisher.publish_namespace("a/b".to_string()).await });
        let SessionEvent::PublishNamespace(handler) = next_event(&server).await else {
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
        let SessionEvent::PublishNamespace(handler) = next_event(&server).await else {
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
    async fn a_second_answer_sends_nothing() {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();
        let (client, server) = spawn_connected_native_sessions("response-guard-answer-once").await;
        let publisher = client.publisher();
        let request =
            tokio::spawn(async move { publisher.publish_namespace("a/b".to_string()).await });
        let SessionEvent::PublishNamespace(handler) = next_event(&server).await else {
            panic!("expected PUBLISH_NAMESPACE from the client");
        };
        handler
            .error(RequestErrorCode::Unauthorized, "denied".to_string())
            .await
            .unwrap();

        // Act
        let second = handler.ok().await;
        let _ = request.await.unwrap();

        // Assert: the first answer wins; the second returns Ok without sending
        assert!(second.is_ok());
        assert_eq!(telemetry.events("PUBLISH_NAMESPACE_ERROR sent").len(), 1);
        assert!(telemetry.events("PUBLISH_NAMESPACE_OK sent").is_empty());
    }

    #[tokio::test]
    async fn dropping_a_clone_while_another_lives_does_not_auto_reject() {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();
        let (client, server) = spawn_connected_native_sessions("response-guard-clone-drop").await;
        let publisher = client.publisher();
        let request =
            tokio::spawn(async move { publisher.publish_namespace("a/b".to_string()).await });
        let SessionEvent::PublishNamespace(handler) = next_event(&server).await else {
            panic!("expected PUBLISH_NAMESPACE from the client");
        };
        let clone = handler.clone();

        // Act
        drop(handler);
        clone.ok().await.unwrap();
        request.await.unwrap().unwrap();

        // Assert
        assert_eq!(telemetry.events("PUBLISH_NAMESPACE_OK sent").len(), 1);
        assert!(telemetry.events("PUBLISH_NAMESPACE_ERROR sent").is_empty());
    }

    #[tokio::test]
    async fn dropping_every_clone_unanswered_auto_rejects_once() {
        // Arrange
        let (telemetry, _guard) = capture_telemetry();
        let (client, server) =
            spawn_connected_native_sessions("response-guard-auto-reject-once").await;
        let publisher = client.publisher();
        let request =
            tokio::spawn(async move { publisher.publish_namespace("a/b".to_string()).await });
        let SessionEvent::PublishNamespace(handler) = next_event(&server).await else {
            panic!("expected PUBLISH_NAMESPACE from the client");
        };
        let clone = handler.clone();

        // Act
        drop(handler);
        drop(clone);
        let _ = request.await.unwrap();

        // Assert
        let sent = telemetry.events("PUBLISH_NAMESPACE_ERROR sent");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].field("error_code"), Some("NotSupported"));
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
        let SessionEvent::PublishNamespace(handler) = next_event(&server).await else {
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
