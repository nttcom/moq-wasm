use std::sync::Arc;

use crate::modules::moqt::{
    control_plane::control_messages::messages::fetch_cancel::FetchCancel,
    domains::{connection::Connection, session_context::SessionContext},
};

#[derive(Clone, Debug)]
pub struct FetchCancelHandler {
    pub(crate) session_context: Arc<SessionContext>,
    request_id: u64,
}

impl FetchCancelHandler {
    pub(crate) fn new(
        session_context: Arc<SessionContext>,
        connection: &Connection,
        fetch_cancel: FetchCancel,
    ) -> Self {
        Self {
            session_context,
            request_id: connection.logical_request_id(fetch_cancel.request_id),
        }
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }
}
