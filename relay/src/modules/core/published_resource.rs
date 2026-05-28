use crate::modules::enums::{FilterType, GroupOrder};

#[derive(Clone)]
pub(crate) struct PublishedResource {
    published_resource: moqt::PublisherInitiatedSubscription,
}

impl PublishedResource {
    pub(crate) fn from(published_resource: moqt::PublisherInitiatedSubscription) -> Self {
        Self { published_resource }
    }

    pub(crate) fn as_moqt(&self) -> &moqt::PublisherInitiatedSubscription {
        &self.published_resource
    }

    pub(crate) fn track_alias(&self) -> u64 {
        self.published_resource.track_alias
    }

    pub(crate) fn filter_type(&self) -> FilterType {
        FilterType::from(self.published_resource.filter_type)
    }

    pub(crate) fn group_order(&self) -> GroupOrder {
        GroupOrder::from(self.published_resource.group_order)
    }
}
