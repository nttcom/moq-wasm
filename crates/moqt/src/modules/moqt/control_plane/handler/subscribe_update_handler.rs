use crate::{
    Location,
    modules::moqt::control_plane::control_messages::messages::subscribe_update::SubscribeUpdate,
};

#[derive(Clone, Debug)]
pub struct SubscribeUpdateHandler {
    request_id: u64,
    subscription_request_id: u64,
    start_location: Location,
    end_group: u64,
    subscriber_priority: u8,
    forward: bool,
    delivery_timeout: Option<u64>,
}

impl SubscribeUpdateHandler {
    pub(crate) fn new(subscribe_update: SubscribeUpdate) -> Self {
        Self {
            request_id: subscribe_update.request_id,
            subscription_request_id: subscribe_update.subscription_request_id,
            start_location: subscribe_update.start_location,
            end_group: subscribe_update.end_group,
            subscriber_priority: subscribe_update.subscriber_priority,
            forward: subscribe_update.forward,
            delivery_timeout: subscribe_update.delivery_timeout,
        }
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    pub fn subscription_request_id(&self) -> u64 {
        self.subscription_request_id
    }

    pub fn start_location(&self) -> Location {
        self.start_location
    }

    pub fn end_group(&self) -> u64 {
        self.end_group
    }

    pub fn subscriber_priority(&self) -> u8 {
        self.subscriber_priority
    }

    pub fn forward(&self) -> bool {
        self.forward
    }

    pub fn delivery_timeout(&self) -> Option<u64> {
        self.delivery_timeout
    }
}
