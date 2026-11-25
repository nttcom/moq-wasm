use crate::{
    RequestFailure,
    modules::moqt::{
        control_plane::enums::{RequestId, ResponseMessage},
        domains::{elapsed_ms, session_context::PublishedNamespaceRecord},
        version::PendingResponse,
    },
};

#[must_use = "dropping this abandons the PUBLISH_NAMESPACE; a late PUBLISH_NAMESPACE_OK is then withdrawn with PUBLISH_NAMESPACE_DONE"]
pub struct PendingPublishNamespace {
    pub(super) request_id: RequestId,
    pub(super) track_namespace: String,
    pub(super) pending: PendingResponse,
    pub(super) record: NamespaceRecord,
}

/// What a `PendingPublishNamespace` holds of its namespace's records.
pub(super) enum NamespaceRecord {
    /// Nothing to forget: a migration's own PUBLISH_NAMESPACE (the migration
    /// holds the write lock that `forget` would wait for), or the
    /// application's before `announce` records it.
    Unrecorded,
    /// The application's PUBLISH_NAMESPACE, unanswered: the records stay until
    /// the answer refuses it, or until the handle is dropped unanswered.
    Recorded(PublishedNamespaceRecord),
    /// `accepted` took the answer and settled the records.
    Answered,
}

impl PendingPublishNamespace {
    /// `Unrecorded` → `Recorded`, once `announce` inserted the records.
    pub(super) fn recorded(&mut self, record: PublishedNamespaceRecord) {
        self.record = NamespaceRecord::Recorded(record);
    }

    pub async fn accepted(mut self) -> Result<(), RequestFailure> {
        let record = std::mem::replace(&mut self.record, NamespaceRecord::Answered);
        let result = self.answer().await;
        if let NamespaceRecord::Recorded(record) = record {
            match &result {
                Err(
                    RequestFailure::Rejected { .. }
                    | RequestFailure::Timeout
                    | RequestFailure::ProtocolViolation(_),
                ) => record.forget().await,
                // A lost connection (`SessionClosed`, `Send`) leaves the
                // record: a migration sends it again.
                _ => record.keep(),
            }
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
