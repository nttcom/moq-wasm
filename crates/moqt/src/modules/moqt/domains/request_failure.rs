use thiserror::Error;

use crate::modules::{
    moqt::control_plane::error_codes::RequestErrorCode,
    transport::{transport_connection::TransportClose, transport_send_stream::TransportSendError},
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
    /// Carries how the transport closed, when it reported that in time.
    #[error("the session closed before the response arrived{}", close_suffix(.0.as_ref()))]
    SessionClosed(Option<TransportClose>),
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
    /// `Publisher::begin_publish_subscribed_namespace` sent nothing (draft-18
    /// §10.18); draft-14 never returns it.
    #[error("no namespace subscription of the peer covers the namespace")]
    NoNamespaceSubscription,
}

/// `: <reason> (code <code>)`, the peer's termination code and reason.
fn close_suffix(close: Option<&TransportClose>) -> String {
    match close {
        None => String::new(),
        Some(TransportClose { code: None, reason }) => format!(": {reason}"),
        Some(TransportClose {
            code: Some(code),
            reason,
        }) if reason.is_empty() => format!(": code {code}"),
        Some(TransportClose {
            code: Some(code),
            reason,
        }) => format!(": {reason} (code {code})"),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn session_closed(code: Option<u32>, reason: &str) -> String {
        RequestFailure::SessionClosed(Some(TransportClose {
            code,
            reason: reason.to_string(),
        }))
        .to_string()
    }

    #[test]
    fn session_closed_reads_the_peers_reason_and_code() {
        // Act / Assert
        assert_eq!(
            session_closed(Some(24), "authorization token expired"),
            "the session closed before the response arrived: authorization token expired (code 24)"
        );
    }

    #[test]
    fn session_closed_reads_the_code_alone_without_a_reason() {
        // Act / Assert
        assert_eq!(
            session_closed(Some(0), ""),
            "the session closed before the response arrived: code 0"
        );
    }

    #[test]
    fn session_closed_reads_a_transport_failure_without_a_code() {
        // Act / Assert
        assert_eq!(
            session_closed(None, "timed out"),
            "the session closed before the response arrived: timed out"
        );
    }

    #[test]
    fn session_closed_reads_the_bare_text_without_a_close() {
        // Act / Assert
        assert_eq!(
            RequestFailure::SessionClosed(None).to_string(),
            "the session closed before the response arrived"
        );
    }
}
