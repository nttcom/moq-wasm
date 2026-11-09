use moqt::{RequestErrorCode, RequestFailure};

pub(crate) fn request_failure_code(error: &RequestFailure) -> (RequestErrorCode, String) {
    match error {
        RequestFailure::Rejected { code, reason, .. } => (*code, reason.clone()),
        RequestFailure::Timeout => (RequestErrorCode::Timeout, error.to_string()),
        _ => (RequestErrorCode::InternalError, error.to_string()),
    }
}
