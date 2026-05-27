use std::sync::Arc;

use crate::{
    TransportProtocol,
    modules::moqt::{
        domains::session_context::SessionContext,
        runtime::dispatch::incoming_object::IncomingObject,
    },
};

pub(crate) struct SubscriptionNotifier;

impl SubscriptionNotifier {
    const MAX_PENDING_OBJECTS_PER_TRACK_ALIAS: usize = 64;

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
        if let Some(sender) = context.notification_map.read().await.get(&track_alias) {
            if let Err(error) = sender.send(incoming_object) {
                tracing::warn!("Failed to notify incoming object: {}", error);
            }
            return;
        }

        let mut pending = context.pending_incoming_objects.lock().await;
        let objects = pending.entry(track_alias).or_default();
        if objects.len() >= Self::MAX_PENDING_OBJECTS_PER_TRACK_ALIAS {
            objects.pop_front();
            tracing::warn!(
                track_alias,
                max_pending_objects = Self::MAX_PENDING_OBJECTS_PER_TRACK_ALIAS,
                "pending incoming object buffer is full; dropping oldest object"
            );
        }
        objects.push_back(incoming_object);
        tracing::info!(
            track_alias,
            pending_objects = objects.len(),
            "buffered incoming object until track alias is registered"
        );
    }
}
