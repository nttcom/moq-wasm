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
    LatestGroup = 0x1,
    LatestObject = 0x2,
    AbsoluteStart = 0x3,
    AbsoluteRange = 0x4,
}

impl FilterType {
    pub(crate) fn from(filter_type: moqt::FilterType) -> Self {
        match filter_type {
            moqt::FilterType::LatestGroup => Self::LatestGroup,
            moqt::FilterType::LatestObject => Self::LatestObject,
            moqt::FilterType::AbsoluteStart => Self::AbsoluteStart,
            moqt::FilterType::AbsoluteRange => Self::AbsoluteRange,
        }
    }

    pub(crate) fn into_moqt(&self) -> moqt::FilterType {
        match self {
            FilterType::LatestGroup => moqt::FilterType::LatestGroup,
            FilterType::LatestObject => moqt::FilterType::LatestObject,
            FilterType::AbsoluteStart => moqt::FilterType::AbsoluteStart,
            FilterType::AbsoluteRange => moqt::FilterType::AbsoluteRange,
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

pub(crate) enum MOQTMessageReceived {
    PublishNameSpace(SessionId, Box<dyn PublishNamespaceHandler>),
    SubscribeNameSpace(SessionId, Box<dyn SubscribeNamespaceHandler>),
    Publish(SessionId, Box<dyn PublishHandler>),
    Subscribe(SessionId, Box<dyn SubscribeHandler>),
    ProtocolViolation(),
}
