use std::sync::Arc;

use crate::{TransportProtocol, modules::moqt::domains::session_context::SessionContext};

#[derive(Debug, Clone)]
pub struct UnsubscribeHandler<T: TransportProtocol> {
    _session_context: Arc<SessionContext<T>>,
    request_id: u64,
}

impl<T: TransportProtocol> UnsubscribeHandler<T> {
    pub(crate) fn new(
        session_context: Arc<SessionContext<T>>,
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
