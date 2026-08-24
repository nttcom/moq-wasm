use crate::modules::moqt::control_plane::control_messages::messages::requests_blocked::RequestsBlocked;

#[derive(Clone, Debug)]
pub struct RequestsBlockedHandler {
    maximum_request_id: u64,
}

impl RequestsBlockedHandler {
    pub(crate) fn new(requests_blocked: RequestsBlocked) -> Self {
        Self {
            maximum_request_id: requests_blocked.maximum_request_id,
        }
    }

    /// The Maximum Request ID the peer is blocked on.
    pub fn maximum_request_id(&self) -> u64 {
        self.maximum_request_id
    }
}
