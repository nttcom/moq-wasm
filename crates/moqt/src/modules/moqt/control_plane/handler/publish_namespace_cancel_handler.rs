use crate::modules::moqt::control_plane::error_codes::RequestErrorCode;

#[derive(Clone, Debug)]
pub struct PublishNamespaceCancelHandler {
    track_namespace: String,
    error_code: RequestErrorCode,
    error_reason: String,
}

impl PublishNamespaceCancelHandler {
    pub(crate) fn new(
        track_namespace: String,
        error_code: RequestErrorCode,
        error_reason: String,
    ) -> Self {
        Self {
            track_namespace,
            error_code,
            error_reason,
        }
    }

    pub fn track_namespace(&self) -> &str {
        &self.track_namespace
    }

    pub fn error_code(&self) -> RequestErrorCode {
        self.error_code
    }

    pub fn error_reason(&self) -> &str {
        &self.error_reason
    }
}
