use moqt::RequestErrorCode;

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
