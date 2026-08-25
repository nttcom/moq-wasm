pub(crate) trait SubscribeUpdateHandler: 'static + Send + Sync {
    fn request_id(&self) -> u64;
    fn subscription_request_id(&self) -> u64;
    fn start_location(&self) -> moqt::Location;
    fn end_group(&self) -> u64;
    fn subscriber_priority(&self) -> u8;
    fn forward(&self) -> bool;
}

impl SubscribeUpdateHandler for moqt::SubscribeUpdateHandler {
    fn request_id(&self) -> u64 {
        self.request_id()
    }

    fn subscription_request_id(&self) -> u64 {
        self.subscription_request_id()
    }

    fn start_location(&self) -> moqt::Location {
        self.start_location()
    }

    fn end_group(&self) -> u64 {
        self.end_group()
    }

    fn subscriber_priority(&self) -> u8 {
        self.subscriber_priority()
    }

    fn forward(&self) -> bool {
        self.forward()
    }
}
