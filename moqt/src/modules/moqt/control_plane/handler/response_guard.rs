use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::{
    TransportProtocol,
    modules::moqt::{
        control_plane::control_messages::{
            control_message_type::ControlMessageType, messages::request_error::RequestError,
        },
        domains::session_context::SessionContext,
    },
};

/// NOT_SUPPORTED shares the code 0x3 across every *_ERROR message
/// (draft-14 §13.1).
const ERROR_NOT_SUPPORTED: u64 = 0x3;

/// Auto-answers a request with `error_type` NOT_SUPPORTED when the last
/// handler clone is dropped without responding. An application that ignores
/// the session event would otherwise leave the requester waiting for its
/// control timeout.
#[derive(Debug, Clone)]
pub(crate) struct ResponseGuard<T: TransportProtocol> {
    session_context: Arc<SessionContext<T>>,
    request_id: u64,
    error_type: ControlMessageType,
    responded: Arc<AtomicBool>,
}

impl<T: TransportProtocol> ResponseGuard<T> {
    pub(crate) fn new(
        session_context: Arc<SessionContext<T>>,
        request_id: u64,
        error_type: ControlMessageType,
    ) -> Self {
        Self {
            session_context,
            request_id,
            error_type,
            responded: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn mark_responded(&self) {
        self.responded.store(true, Ordering::Relaxed);
    }
}

impl<T: TransportProtocol> Drop for ResponseGuard<T> {
    fn drop(&mut self) {
        // strong_count == 1 limits this to the last clone; a single
        // fire-and-forget send needs no owned JoinHandle.
        if self.responded.load(Ordering::Relaxed) || Arc::strong_count(&self.responded) > 1 {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let session_context = self.session_context.clone();
        let request_id = self.request_id;
        let error_type = self.error_type;
        runtime.spawn(async move {
            let err = RequestError {
                request_id,
                error_code: ERROR_NOT_SUPPORTED,
                reason_phrase: "request not handled by application".to_string(),
            };
            if let Err(error) = session_context
                .send_stream
                .send(error_type, err.encode())
                .await
            {
                tracing::warn!(
                    ?error,
                    request_id,
                    ?error_type,
                    "failed to auto-reject unhandled request"
                );
            }
        });
    }
}
