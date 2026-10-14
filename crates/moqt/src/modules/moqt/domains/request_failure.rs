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
    #[error("no response within the request timeout")]
    Timeout,
    #[error("the session closed before the response arrived")]
    SessionClosed,
    #[error("failed to send the request: {0}")]
    Send(#[from] TransportSendError),
    #[error("protocol violation: {0}")]
    ProtocolViolation(&'static str),
}
