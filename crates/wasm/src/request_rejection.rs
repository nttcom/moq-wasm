use moqt::{RequestErrorCode, RequestFailure};

/// The reason phrase sent with a request this client rejects.
pub(crate) fn rejection_reason(code: RequestErrorCode) -> &'static str {
    match code {
        RequestErrorCode::NotSupported => "not supported",
        RequestErrorCode::TrackDoesNotExist => "track does not exist",
        RequestErrorCode::InvalidRange => "invalid range",
        RequestErrorCode::InvalidJoiningRequestId => "invalid joining request id",
        _ => "request rejected",
    }
}

/// The code and reason JavaScript sees for a request the peer or the session
/// failed.
pub(crate) fn request_failure_code(error: &RequestFailure) -> (RequestErrorCode, String) {
    match error {
        RequestFailure::Rejected { code, reason, .. } => (*code, reason.clone()),
        RequestFailure::Timeout => (RequestErrorCode::Timeout, error.to_string()),
        _ => (RequestErrorCode::InternalError, error.to_string()),
    }
}
