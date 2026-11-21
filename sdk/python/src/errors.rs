use moqt::{
    RequestErrorCode, RequestFailure, TransportClose,
    client::RequestError as ClientRequestError,
    wire::{RequestKind, request_error_code_to_wire},
};
use pyo3::{create_exception, exceptions::PyRuntimeError, prelude::*};

create_exception!(
    moq_wasm,
    RequestError,
    PyRuntimeError,
    "A request the peer rejected or that failed, with the draft-14 error code of its message."
);

/// The Python exception of a failed request: a `RequestError` whose message
/// is the `RequestFailure` alone, without the `{kind:?} failed:` prefix the
/// Rust error displays.
pub(crate) fn request_error(error: ClientRequestError) -> PyErr {
    failed_request(error.kind, &error.failure)
}

/// The Python exception of an error from a call that sends a `kind`
/// request among other steps: a `RequestError` when the request failed, and
/// a plain `RuntimeError` for a failure of another step.
pub(crate) fn request_error_in(error: anyhow::Error, kind: RequestKind) -> PyErr {
    match error.downcast::<RequestFailure>() {
        Ok(failure) => failed_request(kind, &failure),
        Err(error) => error.into(),
    }
}

fn failed_request(kind: RequestKind, failure: &RequestFailure) -> PyErr {
    // As `moqt::client::RequestError::code` and `reason` read a failure.
    let (code, reason) = match failure {
        RequestFailure::Rejected { code, reason, .. } => (*code, reason.clone()),
        RequestFailure::Timeout => (RequestErrorCode::Timeout, failure.to_string()),
        failure => (RequestErrorCode::InternalError, failure.to_string()),
    };
    let code = request_error_code_to_wire(code, kind);
    let message_type = message_type(kind);
    let exception = RequestError::new_err(failure.to_string());
    Python::attach(|py| {
        let value = exception.value(py);
        value.setattr("message_type", message_type)?;
        value.setattr("code", code)?;
        value.setattr("reason", reason)?;
        PyResult::Ok(())
    })
    .err()
    .unwrap_or(exception)
}

fn message_type(kind: RequestKind) -> &'static str {
    match kind {
        RequestKind::Subscribe => "SUBSCRIBE",
        RequestKind::Publish => "PUBLISH",
        RequestKind::Fetch => "FETCH",
        RequestKind::TrackStatus => "TRACK_STATUS",
        RequestKind::PublishNamespace => "PUBLISH_NAMESPACE",
        RequestKind::SubscribeNamespace => "SUBSCRIBE_NAMESPACE",
    }
}

/// How the session's last connection closed.
#[pyclass(frozen, get_all, module = "moq_wasm")]
pub struct CloseInfo {
    /// The termination code it closed with; None when the connection was
    /// lost without one.
    pub code: Option<u32>,
    pub reason: String,
}

impl From<TransportClose> for CloseInfo {
    fn from(close: TransportClose) -> Self {
        Self {
            code: close.code,
            reason: close.reason,
        }
    }
}

/// The bytes the session's connections carried.
#[pyclass(frozen, get_all, module = "moq_wasm")]
pub struct TransportStats {
    pub sent_bytes: u64,
    pub received_bytes: u64,
}
