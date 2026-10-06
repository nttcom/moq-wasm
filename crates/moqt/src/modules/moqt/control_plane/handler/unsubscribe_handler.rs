use std::sync::Arc;

use crate::modules::moqt::domains::session_context::SessionContext;

#[derive(Debug, Clone)]
pub struct UnsubscribeHandler {
    _session_context: Arc<SessionContext>,
    request_id: u64,
}

impl UnsubscribeHandler {
    pub(crate) fn new(
        session_context: Arc<SessionContext>,
        unsubscribe_message: crate::modules::moqt::control_plane::control_messages::messages::unsubscribe::Unsubscribe,
    ) -> Self {
        Self {
            _session_context: session_context,
            request_id: unsubscribe_message.request_id,
        }
    }

    pub fn subscribe_id(&self) -> u64 {
        self.request_id
    }
}
