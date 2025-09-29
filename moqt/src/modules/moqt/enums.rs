
#[derive(Clone)]
pub(crate) enum ReceiveEvent {
    Message(Vec<u8>),
    Error()
}

pub enum PublisherEvent {
    PublishNameSpace(u64, Vec<String>),
    Publish()
}

pub enum SubscriberEvent {
    SubscribeNameSpace(u64, Vec<String>),
    Subscribe()
}