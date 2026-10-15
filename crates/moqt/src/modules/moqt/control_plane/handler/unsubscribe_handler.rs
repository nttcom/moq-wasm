use crate::modules::moqt::control_plane::control_messages::messages::unsubscribe::Unsubscribe;

#[derive(Debug, Clone)]
pub struct UnsubscribeHandler {
    request_id: u64,
}

impl UnsubscribeHandler {
    pub(crate) fn new(unsubscribe_message: Unsubscribe) -> Self {
        Self {
            request_id: unsubscribe_message.request_id,
        }
    }

    pub fn subscribe_id(&self) -> u64 {
        self.request_id
    }
}
