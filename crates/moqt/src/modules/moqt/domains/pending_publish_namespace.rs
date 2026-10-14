use crate::{
    RequestFailure,
    modules::moqt::{
        control_plane::enums::{RequestId, ResponseMessage},
        version::PendingResponse,
    },
};

#[must_use = "dropping this abandons the PUBLISH_NAMESPACE; a late PUBLISH_NAMESPACE_OK is then withdrawn with PUBLISH_NAMESPACE_DONE"]
pub struct PendingPublishNamespace {
    pub(super) request_id: RequestId,
    pub(super) pending: PendingResponse,
}

impl PendingPublishNamespace {
    pub async fn accepted(self) -> Result<(), RequestFailure> {
        match self.pending.response().await? {
            ResponseMessage::PublishNamespaceOk(request_id) if request_id == self.request_id => {
                Ok(())
            }
            ResponseMessage::PublishNamespaceError(request_id, error_code, reason_phrase) => {
                Err(RequestFailure::Rejected {
                    request_id,
                    code: error_code,
                    reason: reason_phrase,
                })
            }
            _ => Err(RequestFailure::ProtocolViolation(
                "response type does not match the request",
            )),
        }
    }
}
