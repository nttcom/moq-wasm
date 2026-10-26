use moqt::{
    object::Location,
    subscribe::{FilterType, SubscriptionUpdateRefusal},
};

pub(crate) trait SubscribeUpdateHandler: 'static + Send + Sync {
    fn request_id(&self) -> u64;
    fn subscription_request_id(&self) -> u64;
    fn subscriber_priority(&self) -> u8;
    fn forward(&self) -> bool;
    fn filter(&self) -> Option<FilterType>;
    fn check_widening(&self) -> Result<(), SubscriptionUpdateRefusal>;
    fn ok(&self, largest: Option<Location>) -> bool;
}

impl SubscribeUpdateHandler for moqt::subscribe::SubscribeUpdateHandler {
    fn request_id(&self) -> u64 {
        self.request_id()
    }

    fn subscription_request_id(&self) -> u64 {
        self.subscription_request_id()
    }

    fn subscriber_priority(&self) -> u8 {
        self.subscriber_priority()
    }

    fn forward(&self) -> bool {
        self.forward()
    }

    fn filter(&self) -> Option<FilterType> {
        self.filter()
    }

    fn check_widening(&self) -> Result<(), SubscriptionUpdateRefusal> {
        self.check_widening()
    }

    fn ok(&self, largest: Option<Location>) -> bool {
        self.ok(largest)
    }
}
