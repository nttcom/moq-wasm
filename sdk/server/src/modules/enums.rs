type PublisherId = usize;
type SubscriberId = usize;

pub enum PublisherEvent {
    PublishNameSpace(SubscriberId, u64, Vec<String>),
    Publish()
}

pub enum SubscriberEvent {
    SubscribeNameSpace(PublisherId, u64, Vec<String>),
    Subscribe()
}
