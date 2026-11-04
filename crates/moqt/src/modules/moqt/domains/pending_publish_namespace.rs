use std::time::Duration;

use crate::{
    RequestFailure,
    modules::moqt::{
        control_plane::enums::{RequestId, ResponseMessage},
        domains::elapsed_ms,
        version::PendingResponse,
    },
};

#[must_use = "dropping this abandons the PUBLISH_NAMESPACE; a late PUBLISH_NAMESPACE_OK is then withdrawn with PUBLISH_NAMESPACE_DONE"]
pub struct PendingPublishNamespace {
    pub(super) request_id: RequestId,
    pub(super) track_namespace: String,
    pub(super) sent_at: Duration,
    pub(super) pending: PendingResponse,
}

impl PendingPublishNamespace {
    pub async fn accepted(self) -> Result<(), RequestFailure> {
        let track_namespace = self.track_namespace;
        match self.pending.response().await?.accept() {
            ResponseMessage::PublishNamespaceOk(request_id) if request_id == self.request_id => {
                tracing::info!(
                    request_id,
                    track_namespace,
                    duration_ms = elapsed_ms(self.sent_at),
                    "PUBLISH_NAMESPACE_OK received"
                );
                Ok(())
            }
            ResponseMessage::PublishNamespaceError(request_id, error_code, reason_phrase) => {
                tracing::info!(
                    request_id,
                    track_namespace,
                    ?error_code,
                    %reason_phrase,
                    duration_ms = elapsed_ms(self.sent_at),
                    "PUBLISH_NAMESPACE_ERROR received"
                );
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
