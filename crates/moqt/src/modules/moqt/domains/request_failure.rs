use thiserror::Error;

use crate::modules::{
    moqt::control_plane::error_codes::RequestErrorCode,
    transport::transport_send_stream::TransportSendError,
};

/// The session stays open after `Rejected` and `Timeout` (draft-14 §12.2).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RequestFailure {
    #[error("peer rejected request {request_id} with {code:?}: {reason}")]
    Rejected {
        request_id: u64,
        code: RequestErrorCode,
        reason: String,
    },
    #[error("request timed out")]
    Timeout,
    #[error("the session closed before the response arrived")]
    SessionClosed,
    #[error("failed to send the request: {0}")]
    Send(#[from] TransportSendError),
    #[error("protocol violation: {0}")]
    ProtocolViolation(&'static str),
}

/// Records on the current span the id of the request a response answered:
/// the one `request_id` reads from an `Ok`, or the one a rejection names. A
/// request that was never answered records nothing.
pub(crate) fn record_request_id<T>(
    result: &Result<T, RequestFailure>,
    request_id: impl FnOnce(&T) -> u64,
) {
    let request_id = match result {
        Ok(answered) => request_id(answered),
        Err(RequestFailure::Rejected { request_id, .. }) => *request_id,
        Err(_) => return,
    };
    tracing::Span::current().record("request_id", request_id);
}

/// Records on the current span how a request ended: `ok` when its `*_OK`
/// arrived, `rejected` when the peer answered `*_ERROR`, `failed` otherwise.
pub(crate) fn record_outcome<T>(result: &Result<T, RequestFailure>) {
    let outcome = match result {
        Ok(_) => "ok",
        Err(RequestFailure::Rejected { .. }) => "rejected",
        Err(_) => "failed",
    };
    tracing::Span::current().record("outcome", outcome);
}
