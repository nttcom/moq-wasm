use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::modules::{
    executor,
    moqt::{
        control_plane::error_codes::{RequestErrorCode, RequestKind},
        domains::session_context::SessionContext,
        version::OutgoingResponse,
    },
    transport::transport_send_stream::TransportSendError,
};

/// Sends the `*_ERROR` reply of one request, and auto-answers NOT_SUPPORTED
/// when the last handler clone is dropped without responding. An application
/// that ignores the session event would otherwise leave the requester waiting
/// for its control timeout.
#[derive(Debug, Clone)]
pub(crate) struct ResponseGuard {
    session_context: Arc<SessionContext>,
    request_id: u64,
    kind: RequestKind,
    responded: Arc<AtomicBool>,
}

impl ResponseGuard {
    pub(crate) fn new(
        session_context: Arc<SessionContext>,
        request_id: u64,
        kind: RequestKind,
    ) -> Self {
        Self {
            session_context,
            request_id,
            kind,
            responded: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn mark_responded(&self) {
        self.responded.store(true, Ordering::Relaxed);
    }

    pub(crate) async fn reject(
        &self,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), TransportSendError> {
        self.mark_responded();
        send_request_error(
            &self.session_context,
            self.request_id,
            self.kind,
            code,
            reason_phrase,
        )
        .await
    }
}

async fn send_request_error(
    session_context: &SessionContext,
    request_id: u64,
    kind: RequestKind,
    code: RequestErrorCode,
    reason_phrase: String,
) -> Result<(), TransportSendError> {
    session_context
        .control
        .respond(OutgoingResponse::Error {
            kind,
            request_id,
            code,
            reason_phrase,
        })
        .await
}

impl Drop for ResponseGuard {
    fn drop(&mut self) {
        // strong_count == 1 limits this to the last clone; a single
        // fire-and-forget send needs no owned JoinHandle.
        if self.responded.load(Ordering::Relaxed) || Arc::strong_count(&self.responded) > 1 {
            return;
        }
        let session_context = self.session_context.clone();
        let request_id = self.request_id;
        let kind = self.kind;
        executor::try_spawn("Unhandled Request Rejecter", async move {
            if let Err(error) = send_request_error(
                &session_context,
                request_id,
                kind,
                RequestErrorCode::NotSupported,
                "request not handled by application".to_string(),
            )
            .await
            {
                tracing::warn!(
                    ?error,
                    request_id,
                    ?kind,
                    "failed to auto-reject unhandled request"
                );
            }
        });
    }
}
