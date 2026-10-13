use thiserror::Error;

use crate::{modules::transport::transport_send_stream::TransportSendError, wire::RequestError};

/// The session stays open after `Rejected` and `Timeout` (draft-14 §12.2).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RequestFailure {
    #[error("peer rejected request {request_id} with code {code:#x}: {reason}")]
    Rejected {
        request_id: u64,
        code: u64,
        reason: String,
    },
    #[error("no response within the request timeout")]
    Timeout,
    #[error("the session closed before the response arrived")]
    SessionClosed,
    #[error("failed to send the request: {0}")]
    Send(#[from] TransportSendError),
    #[error("protocol violation: {0}")]
    ProtocolViolation(&'static str),
}

impl From<RequestError> for RequestFailure {
    fn from(error: RequestError) -> Self {
        Self::Rejected {
            request_id: error.request_id,
            code: error.error_code,
            reason: error.reason_phrase,
        }
    }
}
