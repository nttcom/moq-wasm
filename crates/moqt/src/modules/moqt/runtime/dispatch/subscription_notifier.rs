use std::sync::Arc;

use crate::modules::moqt::{
    domains::session_context::{IncomingObjectNotification, SessionContext},
    runtime::dispatch::incoming_object::IncomingObject,
};

pub(crate) struct SubscriptionNotifier;

impl SubscriptionNotifier {
    #[tracing::instrument(
        level = "trace",
        name = "moqt.subscription_notifier.notify",
        skip_all,
        fields(track_alias = track_alias)
    )]
    pub(crate) async fn notify(
        context: &Arc<SessionContext>,
        track_alias: u64,
        incoming_object: IncomingObject,
    ) {
        match context
            .notify_incoming_object(track_alias, incoming_object)
            .await
        {
            IncomingObjectNotification::Notified => {}
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
                tracing::trace!(
                    track_alias,
                    pending_objects,
                    "buffered incoming object until track alias is registered"
                );
            }
            IncomingObjectNotification::ReceiverClosed => {
                tracing::debug!(
                    track_alias,
                    "dropping incoming object: its receiver is closed"
                );
            }
            IncomingObjectNotification::Discarded => {
                tracing::trace!(
                    track_alias,
                    "discarding incoming object of a cancelled subscription"
                );
            }
        }
    }
}
