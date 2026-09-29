/// SUBSCRIBE_ERROR codes (draft-14 §9.9), which TRACK_STATUS_ERROR reuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestRejection {
    NotSupported,
    TrackDoesNotExist,
}

impl RequestRejection {
    pub(crate) fn code(self) -> u64 {
        match self {
            Self::NotSupported => 0x3,
            Self::TrackDoesNotExist => 0x4,
        }
    }

    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::NotSupported => "not supported",
            Self::TrackDoesNotExist => "track does not exist",
        }
    }
}
