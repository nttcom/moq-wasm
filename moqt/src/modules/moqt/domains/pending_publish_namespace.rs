use std::sync::Arc;

use anyhow::bail;

use crate::{
    TransportProtocol,
    modules::moqt::{
        control_plane::enums::{RequestId, ResponseMessage},
        domains::session_context::{RegisteredSender, SessionContext},
    },
    wire::RequestError,
};

#[must_use = "dropping this abandons the PUBLISH_NAMESPACE; a late PUBLISH_NAMESPACE_OK is then withdrawn with PUBLISH_NAMESPACE_DONE"]
pub struct PendingPublishNamespace<T: TransportProtocol> {
    pub(super) session: Arc<SessionContext<T>>,
    pub(super) request_id: RequestId,
    pub(super) receiver: tokio::sync::oneshot::Receiver<ResponseMessage>,
    pub(super) _registered_sender: RegisteredSender<T>,
}

impl<T: TransportProtocol> PendingPublishNamespace<T> {
    pub async fn accepted(self) -> anyhow::Result<()> {
        match self.session.await_response(self.receiver).await? {
            ResponseMessage::PublishNamespaceOk(request_id) if request_id == self.request_id => {
                Ok(())
            }
            ResponseMessage::PublishNamespaceError(request_id, error_code, reason_phrase) => {
                Err(RequestError {
                    request_id,
                    error_code,
                    reason_phrase,
                }
                .into())
            }
            _ => bail!("Protocol violation"),
        }
    }
}
