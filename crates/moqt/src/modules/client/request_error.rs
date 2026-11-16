use thiserror::Error;

use crate::{RequestErrorCode, RequestFailure, wire::RequestKind};

/// A failed request; `code` gives the draft's error code for it.
#[derive(Debug, Error)]
#[non_exhaustive]
#[error("{kind:?} failed: {failure}")]
pub struct RequestError {
    pub kind: RequestKind,
    pub failure: RequestFailure,
}

/// A FETCH that did not succeed: the peer did not accept it (`Request`), or
/// its response stream did not arrive (`Stream`), as when the session ends
/// first.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FetchError {
    #[error(transparent)]
    Request(#[from] RequestError),
    #[error(transparent)]
    Stream(anyhow::Error),
}

impl RequestError {
    pub(super) fn of(kind: RequestKind) -> impl FnOnce(RequestFailure) -> Self {
        move |failure| Self { kind, failure }
    }

    /// The code the peer rejected the request with, TIMEOUT when no response
    /// came, and INTERNAL_ERROR for a failure on this side or of the session.
    pub fn code(&self) -> RequestErrorCode {
        match &self.failure {
            RequestFailure::Rejected { code, .. } => *code,
            RequestFailure::Timeout => RequestErrorCode::Timeout,
            _ => RequestErrorCode::InternalError,
        }
    }

    /// The peer's reason phrase for a rejection, or a description of the
    /// failure.
    pub fn reason(&self) -> String {
        match &self.failure {
            RequestFailure::Rejected { reason, .. } => reason.clone(),
            failure => failure.to_string(),
        }
    }
}
