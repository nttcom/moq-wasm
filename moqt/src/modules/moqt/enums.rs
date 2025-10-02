#[derive(Clone)]
pub(crate) enum ReceiveEvent {
    Message(Vec<u8>),
    Error(),
}

pub enum SessionEvent {
    PublishNameSpace(u64, Vec<String>),
    SubscribeNameSpace(u64, Vec<String>),
    Publish(),
    Subscribe(),
}
