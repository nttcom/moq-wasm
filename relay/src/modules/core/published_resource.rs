use crate::modules::enums::{FilterType, GroupOrder};

pub(crate) struct PublishedResource {
    published_resource: moqt::PublishedResource,
}

impl PublishedResource {
    pub(crate) fn from(published_resource: moqt::PublishedResource) -> Self {
        Self { published_resource }
    }

    pub(crate) fn as_moqt(&self) -> &moqt::PublishedResource {
        &self.published_resource
    }

    pub(crate) fn group_order(&self) -> GroupOrder {
        GroupOrder::from(self.published_resource.group_order)
    }

    pub(crate) fn filter_type(&self) -> FilterType {
        FilterType::from(self.published_resource.filter_type)
    }
}
