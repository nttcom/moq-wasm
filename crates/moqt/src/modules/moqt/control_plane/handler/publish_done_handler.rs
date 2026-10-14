use crate::modules::moqt::control_plane::error_codes::PublishDoneCode;

#[derive(Clone, Debug)]
pub struct PublishDoneHandler {
    request_id: u64,
    status_code: PublishDoneCode,
    stream_count: u64,
    error_reason: String,
}

impl PublishDoneHandler {
    pub(crate) fn new(
        request_id: u64,
        status_code: PublishDoneCode,
        stream_count: u64,
        error_reason: String,
    ) -> Self {
        Self {
            request_id,
            status_code,
            stream_count,
            error_reason,
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
