
#[derive(Clone)]
pub(crate) enum ReceiveEvent {
    Message(Vec<u8>),
    Error()
}