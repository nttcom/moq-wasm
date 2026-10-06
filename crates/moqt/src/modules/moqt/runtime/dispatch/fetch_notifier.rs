use std::sync::Arc;

use crate::modules::moqt::{
    domains::session_context::SessionContext, runtime::dispatch::incoming_object::IncomingObject,
};

pub(crate) struct FetchNotifier;

impl FetchNotifier {
    #[tracing::instrument(
        level = "info",
        name = "moqt.fetch_notifier.notify",
        skip_all,
        fields(request_id = request_id)
    )]
    pub(crate) async fn notify(
        context: &Arc<SessionContext>,
        request_id: u64,
        incoming_object: IncomingObject,
    ) {
        // Draft-14 §9.16.3: a FETCH response is delivered on a single stream.
        let sender = context
            .fetch_notification_map
            .lock()
            .await
            .remove(&request_id);
        if let Some(sender) = sender {
            if let Err(e) = sender.send(incoming_object) {
                tracing::warn!("Failed to notify fetch stream: {}", e);
            }
        } else {
            tracing::error!("No sender found for request_id: {}", request_id);
        }
    }
}
