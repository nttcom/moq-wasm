use std::sync::Arc;

use crate::{
    TransportProtocol,
    modules::moqt::{
        domains::session_context::{IncomingObjectNotification, SessionContext},
        runtime::dispatch::incoming_object::IncomingObject,
    },
};

pub(crate) struct SubscriptionNotifier;

impl SubscriptionNotifier {
    #[tracing::instrument(
        level = "info",
        name = "moqt.subscription_notifier.notify",
        skip_all,
        fields(track_alias = track_alias)
    )]
    pub(crate) async fn notify<T: TransportProtocol>(
        context: &Arc<SessionContext<T>>,
        track_alias: u64,
        incoming_object: IncomingObject<T>,
    ) {
        match context
            .notify_incoming_object(track_alias, incoming_object)
            .await
        {
            IncomingObjectNotification::Notified => {
                tracing::debug!(track_alias, "notifying registered incoming object receiver");
            }
            IncomingObjectNotification::Buffered {
                pending_objects,
                dropped_oldest,
            } => {
                if dropped_oldest {
                    tracing::warn!(
                        track_alias,
                        pending_objects,
                        "pending incoming object buffer is full; dropping oldest object"
                    );
                }
                tracing::debug!(
                    track_alias,
                    pending_objects,
                    "buffered incoming object until track alias is registered"
                );
            }
            IncomingObjectNotification::ReceiverClosed => {
                tracing::warn!("Failed to notify incoming object: receiver closed");
            }
            IncomingObjectNotification::Discarded => {
                tracing::debug!(
                    track_alias,
                    "discarding incoming object of a cancelled subscription"
                );
            }
        }
    }
}
