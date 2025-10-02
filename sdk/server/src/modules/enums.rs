use uuid::Uuid;

pub enum SessionEvent {
    PublishNameSpace(Uuid, u64, Vec<String>),
    SubscribeNameSpace(Uuid, u64, Vec<String>),
    Publish(),
    Subscribe()
}