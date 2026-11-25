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
    /// This side withdrew the request before its response arrived (e.g.
    /// PUBLISH_NAMESPACE_DONE on a pending PUBLISH_NAMESPACE, draft-18 §3.3.2).
    /// The session stays open.
    #[error("the request was withdrawn before its response arrived")]
    Withdrawn,
    #[error("failed to send the request: {0}")]
    Send(#[from] TransportSendError),
    #[error("protocol violation: {0}")]
    ProtocolViolation(&'static str),
    /// The track carries a Mandatory Track Property the peer's draft cannot
    /// carry (draft-14 has no Track Properties; draft-18 §2.5.1), so the
    /// request was not sent.
    #[error("the peer cannot understand the track's Mandatory Track Properties")]
    TrackNotUnderstood,
    /// A PUBLISH for a track the peer's SUBSCRIBE_TRACKS covers was not
    /// sent: the peer's stream limit was reached, so PUBLISH_BLOCKED went on
    /// that SUBSCRIBE_TRACKS, or PUBLISH_BLOCKED had already gone out for the
    /// track on it. No PUBLISH for the track follows while that
    /// SUBSCRIBE_TRACKS lives (draft-18 §6.1 MUST NOT). draft-14 never fails
    /// so.
    #[error("no stream for the PUBLISH; PUBLISH_BLOCKED was sent")]
    PublishBlocked,
}

pub(crate) fn record_outcome<T>(result: &Result<T, RequestFailure>) {
    let outcome = match result {
        Ok(_) => "ok",
        Err(RequestFailure::Rejected { .. }) => "rejected",
        Err(RequestFailure::Timeout) => "timeout",
        Err(_) => "failed",
    };
    tracing::Span::current().record("outcome", outcome);
}
