use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::{
    TransportProtocol,
    modules::{
        moqt::{
            control_plane::{
                control_messages::{
                    control_message_type::ControlMessageType, messages::request_error::RequestError,
                },
                error_codes::{RequestErrorCode, RequestKind},
            },
            domains::session_context::SessionContext,
        },
        transport::transport_send_stream::TransportSendError,
    },
};

/// Auto-answers a request with NOT_SUPPORTED when the last handler clone is
/// dropped without responding. An application that ignores the session event
/// would otherwise leave the requester waiting for its control timeout.
#[derive(Debug, Clone)]
pub(crate) struct ResponseGuard<T: TransportProtocol> {
    session_context: Arc<SessionContext<T>>,
    request_id: u64,
    kind: RequestKind,
    responded: Arc<AtomicBool>,
}

impl<T: TransportProtocol> ResponseGuard<T> {
    pub(crate) fn new(
        session_context: Arc<SessionContext<T>>,
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

async fn send_request_error<T: TransportProtocol>(
    session_context: &SessionContext<T>,
    request_id: u64,
    kind: RequestKind,
    code: RequestErrorCode,
    reason_phrase: String,
) -> Result<(), TransportSendError> {
    let err = RequestError {
        request_id,
        error_code: code.wire_value(kind),
        reason_phrase,
    };
    session_context
        .send_stream
        .send(error_message_type(kind), err.encode())
        .await
}

fn error_message_type(kind: RequestKind) -> ControlMessageType {
    match kind {
        RequestKind::Subscribe => ControlMessageType::SubscribeError,
        RequestKind::Publish => ControlMessageType::PublishError,
        RequestKind::Fetch => ControlMessageType::FetchError,
        RequestKind::TrackStatus => ControlMessageType::TrackStatusError,
        RequestKind::PublishNamespace => ControlMessageType::PublishNamespaceError,
        RequestKind::SubscribeNamespace => ControlMessageType::SubscribeNamespaceError,
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
        let kind = self.kind;
        runtime.spawn(async move {
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
