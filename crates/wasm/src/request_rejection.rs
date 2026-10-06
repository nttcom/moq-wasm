/// Error codes shared by SUBSCRIBE_ERROR (draft-14 §9.9), which
/// TRACK_STATUS_ERROR reuses, and FETCH_ERROR (§9.18); INVALID_RANGE and
/// INVALID_JOINING_REQUEST_ID exist for FETCH_ERROR only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestRejection {
    NotSupported,
    TrackDoesNotExist,
    InvalidRange,
    InvalidJoiningRequestId,
}

impl RequestRejection {
    pub(crate) fn code_and_reason(self) -> (u64, &'static str) {
        match self {
            Self::NotSupported => (0x3, "not supported"),
            Self::TrackDoesNotExist => (0x4, "track does not exist"),
            Self::InvalidRange => (0x5, "invalid range"),
            Self::InvalidJoiningRequestId => (0x7, "invalid joining request id"),
        }
    }
}
