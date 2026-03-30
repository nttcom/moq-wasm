use std::any::Any;

use crate::modules::enums::ContentExists;

pub trait Subscription: 'static + Send + Sync {
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn track_alias(&self) -> u64;
    fn expires(&self) -> u64;
    fn content_exists(&self) -> ContentExists;
}

impl<T: moqt::TransportProtocol> Subscription for moqt::Subscription<T> {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn track_alias(&self) -> u64 {
        self.track_alias
    }

    fn expires(&self) -> u64 {
        self.expires
    }

    fn content_exists(&self) -> ContentExists {
        ContentExists::from(self.content_exists)
    }
}
