use moqt::TransportSendError;

/// Whether a send failed because the session it was sent on is gone, rather
/// than because of one stream or request.
pub(crate) fn is_session_closed(error: &TransportSendError) -> bool {
    matches!(
        error,
        TransportSendError::ConnectionLost { .. } | TransportSendError::SessionError { .. }
    )
}
