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
    const MAX_PENDING_OBJECTS_PER_TRACK_ALIAS: usize = 256;

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
            .notify_incoming_object(
                track_alias,
                incoming_object,
                Self::MAX_PENDING_OBJECTS_PER_TRACK_ALIAS,
            )
            .await
        {
            IncomingObjectNotification::Notified => {
                tracing::info!(track_alias, "notifying registered incoming object receiver");
            }
            IncomingObjectNotification::Buffered {
                pending_objects,
                dropped_oldest,
            } => {
                if dropped_oldest {
                    tracing::warn!(
                        track_alias,
                        max_pending_objects = Self::MAX_PENDING_OBJECTS_PER_TRACK_ALIAS,
                        "pending incoming object buffer is full; dropping oldest object"
                    );
                }
                tracing::info!(
                    track_alias,
                    pending_objects,
                    "buffered incoming object until track alias is registered"
                );
            }
            IncomingObjectNotification::ReceiverClosed => {
                tracing::warn!("Failed to notify incoming object: receiver closed");
            }
        }
    }
}
