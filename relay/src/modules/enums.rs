#[derive(Clone, Debug)]
pub(crate) struct Location {
    pub(crate) group_id: u64,
    pub(crate) object_id: u64,
}

impl Location {
    pub(crate) fn from(location: moqt::Location) -> Self {
        Self {
            group_id: location.group_id,
            object_id: location.object_id,
        }
    }

    pub(crate) fn as_moqt(&self) -> moqt::Location {
        moqt::Location {
            group_id: self.group_id,
            object_id: self.object_id,
        }
    }
}

#[derive(Clone)]
pub(crate) enum FilterType {
    NextGroupStart,
    LargestObject,
    AbsoluteStart { location: Location },
    AbsoluteRange { location: Location, end_group: u64 },
}

impl FilterType {
    pub(crate) fn from(filter_type: moqt::FilterType) -> Self {
        match filter_type {
            moqt::FilterType::NextGroupStart => Self::NextGroupStart,
            moqt::FilterType::LargestObject => Self::LargestObject,
            moqt::FilterType::AbsoluteStart { location } => Self::AbsoluteStart {
                location: Location::from(location),
            },
            moqt::FilterType::AbsoluteRange {
                location,
                end_group,
            } => Self::AbsoluteRange {
                location: Location::from(location),
                end_group,
            },
        }
    }

    pub(crate) fn as_moqt(&self) -> moqt::FilterType {
        match self {
            FilterType::NextGroupStart => moqt::FilterType::NextGroupStart,
            FilterType::LargestObject => moqt::FilterType::LargestObject,
            FilterType::AbsoluteStart { location } => moqt::FilterType::AbsoluteStart {
                location: location.as_moqt(),
            },
            FilterType::AbsoluteRange {
                location,
                end_group,
            } => moqt::FilterType::AbsoluteRange {
                location: location.as_moqt(),
                end_group: *end_group,
            },
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum GroupOrder {
    Publisher = 0x0,
    Ascending = 0x1,
    Descending = 0x2,
}

impl GroupOrder {
    #[allow(dead_code)]
    pub(crate) fn from(group_order: moqt::GroupOrder) -> Self {
        match group_order {
            moqt::GroupOrder::Publisher => Self::Publisher,
            moqt::GroupOrder::Ascending => Self::Ascending,
            moqt::GroupOrder::Descending => Self::Descending,
        }
    }

    pub(crate) fn as_moqt(&self) -> moqt::GroupOrder {
        match self {
            Self::Publisher => moqt::GroupOrder::Publisher,
            Self::Ascending => moqt::GroupOrder::Ascending,
            Self::Descending => moqt::GroupOrder::Descending,
        }
    }
}

// https://www.ietf.org/archive/id/draft-ietf-moq-transport-14.html#section-9.18
// FETCH_ERROR error codes.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub(crate) enum FetchErrorCode {
    InternalError = 0x0,
    Unauthorized = 0x1,
    Timeout = 0x2,
    NotSupported = 0x3,
    TrackDoesNotExist = 0x4,
    InvalidRange = 0x5,
    NoObjects = 0x6,
    InvalidJoiningRequestId = 0x7,
    UnknownStatusInRange = 0x8,
    MalformedTrack = 0x9,
    MalformedAuthToken = 0x10,
    ExpiredAuthToken = 0x12,
}

#[derive(Clone, Debug)]
pub(crate) enum ContentExists {
    False,
    True { location: Location },
}

impl ContentExists {
    pub(crate) fn from(content_exists: moqt::ContentExists) -> Self {
        match content_exists {
            moqt::ContentExists::False => Self::False,
            moqt::ContentExists::True { location } => Self::True {
                location: Location::from(location),
            },
        }
    }

    pub(crate) fn as_moqt(&self) -> moqt::ContentExists {
        match self {
            ContentExists::False => moqt::ContentExists::False,
            ContentExists::True { location } => moqt::ContentExists::True {
                location: location.as_moqt(),
            },
        }
    }

    pub(crate) fn location(&self) -> Option<moqt::Location> {
        match self {
            ContentExists::False => None,
            ContentExists::True { location } => Some(location.as_moqt()),
        }
    }
}
