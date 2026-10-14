use std::sync::Arc;

use crate::modules::moqt::control_plane::error_codes::RequestKind;

use crate::{
    RequestFailure,
    modules::moqt::{
        control_plane::enums::{RequestId, ResponseMessage},
        domains::session_context::{RegisteredSender, SessionContext},
    },
};

#[must_use = "dropping this abandons the PUBLISH_NAMESPACE; a late PUBLISH_NAMESPACE_OK is then withdrawn with PUBLISH_NAMESPACE_DONE"]
pub struct PendingPublishNamespace {
    pub(super) session: Arc<SessionContext>,
    pub(super) request_id: RequestId,
    pub(super) receiver: tokio::sync::oneshot::Receiver<ResponseMessage>,
    pub(super) _registered_sender: RegisteredSender,
}

impl PendingPublishNamespace {
    pub async fn accepted(self) -> Result<(), RequestFailure> {
        match self.session.await_response(self.receiver).await? {
            ResponseMessage::PublishNamespaceOk(request_id) if request_id == self.request_id => {
                Ok(())
            }
            ResponseMessage::PublishNamespaceError(request_id, error_code, reason_phrase) => {
                Err(RequestFailure::rejected(
                    RequestKind::PublishNamespace,
                    request_id,
                    error_code,
                    reason_phrase,
                ))
            }
            _ => Err(RequestFailure::ProtocolViolation(
                "response type does not match the request",
            )),
        }
    }
}
