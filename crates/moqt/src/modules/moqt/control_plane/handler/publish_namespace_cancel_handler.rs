use crate::modules::moqt::control_plane::{
    control_messages::messages::publish_namespace_cancel::PublishNamespaceCancel,
    error_codes::{RequestErrorCode, RequestKind},
};

#[derive(Clone, Debug)]
pub struct PublishNamespaceCancelHandler {
    track_namespace: String,
    error_code: RequestErrorCode,
    error_reason: String,
}

impl PublishNamespaceCancelHandler {
    pub(crate) fn new(publish_namespace_cancel: PublishNamespaceCancel) -> Self {
        Self {
            track_namespace: publish_namespace_cancel.track_namespace.join("/"),
            error_code: RequestErrorCode::from_wire(
                publish_namespace_cancel.error_code,
                RequestKind::PublishNamespace,
            ),
            error_reason: publish_namespace_cancel.error_reason,
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
