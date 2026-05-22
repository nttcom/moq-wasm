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
        let mut count = 0;
        loop {
            if let Some(sender) = context.notification_map.read().await.get(&track_alias) {
                if let Err(error) = sender.send(incoming_object) {
                    tracing::warn!("Failed to notify incoming object: {}", error);
                }
                break;
            }

            if count > 10 {
                tracing::error!(
                    "No sender found for track alias: {} after multiple attempts",
                    track_alias
                );
                break;
            }

            tracing::warn!("No sender found for track alias: {}", track_alias);
            count += 1;
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
}
