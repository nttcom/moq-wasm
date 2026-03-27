use crate::modules::enums::{ContentExists, GroupOrder};

pub trait Subscription: 'static + Send + Sync {
    fn track_alias(&self) -> u64;
    fn expires(&self) -> u64;
    fn group_order(&self) -> GroupOrder;
    fn content_exists(&self) -> ContentExists;
}

impl<T: moqt::TransportProtocol> Subscription for moqt::Subscription<T> {
    fn track_alias(&self) -> u64 {
        self.track_alias
    }

    fn expires(&self) -> u64 {
        self.expires
    }

    fn group_order(&self) -> GroupOrder {
        GroupOrder::from(self.group_order)
    }

    fn content_exists(&self) -> ContentExists {
        ContentExists::from(self.content_exists)
    }
}
