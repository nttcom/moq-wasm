use crate::modules::moqt::control_plane::control_messages::messages::max_request_id::MaxRequestId;

#[derive(Clone, Debug)]
pub struct MaxRequestIdHandler {
    request_id: u64,
}

impl MaxRequestIdHandler {
    pub(crate) fn new(max_request_id: MaxRequestId) -> Self {
        Self {
            request_id: max_request_id.request_id,
        }
    }

    /// The new Maximum Request ID for the session, plus 1.
    pub fn request_id(&self) -> u64 {
        self.request_id
    }
}
