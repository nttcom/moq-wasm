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
    pub(crate) fn code(self) -> u64 {
        match self {
            Self::NotSupported => 0x3,
            Self::TrackDoesNotExist => 0x4,
            Self::InvalidRange => 0x5,
            Self::InvalidJoiningRequestId => 0x7,
        }
    }

    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::NotSupported => "not supported",
            Self::TrackDoesNotExist => "track does not exist",
            Self::InvalidRange => "invalid range",
            Self::InvalidJoiningRequestId => "invalid joining request id",
        }
    }
}
