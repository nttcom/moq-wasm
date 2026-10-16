use std::sync::Arc;

use crate::modules::moqt::{
    control_plane::control_messages::messages::unsubscribe::Unsubscribe,
    domains::{connection::Connection, session_context::SessionContext},
};

#[derive(Debug, Clone)]
pub struct UnsubscribeHandler {
    pub(crate) session_context: Arc<SessionContext>,
    request_id: u64,
}

impl UnsubscribeHandler {
    pub(crate) fn new(
        session_context: Arc<SessionContext>,
        connection: &Connection,
        unsubscribe_message: Unsubscribe,
    ) -> Self {
        Self::ended(
            session_context,
            connection.logical_request_id(unsubscribe_message.request_id),
        )
    }

    /// A subscription that ended with the connection it was served on.
    pub(crate) fn ended(session_context: Arc<SessionContext>, request_id: u64) -> Self {
        Self {
            session_context,
            request_id,
        }
    }

    pub fn subscribe_id(&self) -> u64 {
        self.request_id
    }
}
