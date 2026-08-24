use crate::modules::moqt::control_plane::control_messages::messages::fetch_cancel::FetchCancel;

#[derive(Clone, Debug)]
pub struct FetchCancelHandler {
    request_id: u64,
}

impl FetchCancelHandler {
    pub(crate) fn new(fetch_cancel: FetchCancel) -> Self {
        Self {
            request_id: fetch_cancel.request_id,
        }
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }
}
