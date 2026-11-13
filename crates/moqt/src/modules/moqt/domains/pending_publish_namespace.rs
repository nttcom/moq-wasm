use crate::{
    RequestFailure,
    modules::moqt::{
        control_plane::enums::{RequestId, ResponseMessage},
        domains::{elapsed_ms, session_context::CarriedPublishedNamespace},
        version::PendingResponse,
    },
};

#[must_use = "dropping this abandons the PUBLISH_NAMESPACE; a late PUBLISH_NAMESPACE_OK is then withdrawn with PUBLISH_NAMESPACE_DONE"]
pub struct PendingPublishNamespace {
    pub(super) request_id: RequestId,
    pub(super) track_namespace: String,
    pub(super) pending: PendingResponse,
    /// `None` unless `announce` sent the request in a session that follows
    /// GOAWAY. A migration's own PUBLISH_NAMESPACE must stay `None`: the
    /// migration holds the write lock that `forget` would wait for.
    pub(super) carried: Option<CarriedPublishedNamespace>,
}

impl PendingPublishNamespace {
    pub async fn accepted(mut self) -> Result<(), RequestFailure> {
        let carried = self.carried.take();
        let result = self.answer().await;
        // A lost connection (`SessionClosed`, `Send`) leaves the record: a
        // migration sends it again.
        if let (
            Err(
                RequestFailure::Rejected { .. }
                | RequestFailure::Timeout
                | RequestFailure::ProtocolViolation(_),
            ),
            Some(carried),
        ) = (&result, carried)
        {
            carried.forget().await;
        }
        result
    }

    async fn answer(self) -> Result<(), RequestFailure> {
        let track_namespace = self.track_namespace;
        let sent_at = self.pending.sent_at();
        match self.pending.response().await?.accept() {
            ResponseMessage::PublishNamespaceOk(request_id) if request_id == self.request_id => {
                tracing::info!(
                    request_id,
                    track_namespace,
                    duration_ms = elapsed_ms(sent_at),
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
                    duration_ms = elapsed_ms(sent_at),
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
