use crate::modules::moqt::control_plane::{
    control_messages::messages::publish_done::PublishDone, error_codes::PublishDoneCode,
};

#[derive(Clone, Debug)]
pub struct PublishDoneHandler {
    request_id: u64,
    status_code: PublishDoneCode,
    stream_count: u64,
    error_reason: String,
}

impl PublishDoneHandler {
    pub(crate) fn new(publish_done: PublishDone) -> Self {
        Self {
            request_id: publish_done.request_id,
            status_code: PublishDoneCode::from_wire(publish_done.status_code),
            stream_count: publish_done.stream_count,
            error_reason: publish_done.error_reason,
        }
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    pub fn status_code(&self) -> PublishDoneCode {
        self.status_code
    }

    pub fn stream_count(&self) -> u64 {
        self.stream_count
    }

    pub fn error_reason(&self) -> &str {
        &self.error_reason
    }
}
