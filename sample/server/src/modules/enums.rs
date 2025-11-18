use crate::modules::{
    core::handler::{
        publish::PublishHandler, publish_namespace::PublishNamespaceHandler,
        subscribe::SubscribeHandler, subscribe_namespace::SubscribeNamespaceHandler,
    },
    types::SessionId,
};

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

    pub(crate) fn into_moqt(&self) -> moqt::Location {
        moqt::Location {
            group_id: self.group_id,
            object_id: self.object_id,
        }
    }
}

pub(crate) enum FilterType {
    LatestGroup,
    LatestObject,
    AbsoluteStart { location: Location },
    AbsoluteRange { location: Location, end_group: u64 },
}

impl FilterType {
    pub(crate) fn from(filter_type: moqt::FilterType) -> Self {
        match filter_type {
            moqt::FilterType::LatestGroup => Self::LatestGroup,
            moqt::FilterType::LatestObject => Self::LatestObject,
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

    pub(crate) fn into_moqt(&self) -> moqt::FilterType {
        match self {
            FilterType::LatestGroup => moqt::FilterType::LatestGroup,
            FilterType::LatestObject => moqt::FilterType::LatestObject,
            FilterType::AbsoluteStart { location } => moqt::FilterType::AbsoluteStart {
                location: location.into_moqt(),
            },
            FilterType::AbsoluteRange {
                location,
                end_group,
            } => moqt::FilterType::AbsoluteRange {
                location: location.into_moqt(),
                end_group: *end_group,
            },
        }
    }
}

pub(crate) enum GroupOrder {
    Publisher = 0x0,
    Ascending = 0x1,
    Descending = 0x2,
}

impl GroupOrder {
    pub(crate) fn from(group_order: moqt::GroupOrder) -> Self {
        match group_order {
            moqt::GroupOrder::Publisher => Self::Publisher,
            moqt::GroupOrder::Ascending => Self::Ascending,
            moqt::GroupOrder::Descending => Self::Descending,
        }
    }

    pub(crate) fn into_moqt(&self) -> moqt::GroupOrder {
        match self {
            Self::Publisher => moqt::GroupOrder::Publisher,
            Self::Ascending => moqt::GroupOrder::Ascending,
            Self::Descending => moqt::GroupOrder::Descending,
        }
    }
}

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

    pub(crate) fn into_moqt(&self) -> moqt::ContentExists {
        match self {
            ContentExists::False => moqt::ContentExists::False,
            ContentExists::True { location } => moqt::ContentExists::True {
                location: location.into_moqt(),
            },
        }
    }
}

pub(crate) enum MOQTMessageReceived {
    PublishNameSpace(SessionId, Box<dyn PublishNamespaceHandler>),
    SubscribeNameSpace(SessionId, Box<dyn SubscribeNamespaceHandler>),
    Publish(SessionId, Box<dyn PublishHandler>),
    Subscribe(SessionId, Box<dyn SubscribeHandler>),
    ProtocolViolation(),
}
