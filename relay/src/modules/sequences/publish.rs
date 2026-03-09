use std::sync::Arc;

use uuid::Uuid;

use crate::modules::{
    core::handler::publish::PublishHandler,
    enums::FilterType,
    sequences::{notifier::Notifier, tables::table::Table},
};

pub(crate) struct Publish;

impl Publish {
    pub(crate) async fn handle(
        &self,
        session_id: Uuid,
        table: &dyn Table,
        notifier: &Notifier,
        handler: Box<dyn PublishHandler>,
    ) {
        tracing::info!("SequenceHandler::publish: {}", session_id);
        self.broadcast_to_subscribers(notifier, table, handler.as_ref())
            .await;
        self.register_if_response_succeeded(session_id, table, handler)
            .await;
        tracing::info!("SequenceHandler::publish: {} DONE", session_id);
    }

    async fn broadcast_to_subscribers(
        &self,
        notifier: &Notifier,
        table: &dyn Table,
        handler: &dyn PublishHandler,
    ) {
        let track_namespace = handler.track_namespace().to_string();
        let track_name = handler.track_name().to_string();
        let track_alias = handler.track_alias();

        let full_track_namespace = format!("{}:{}", track_namespace, track_name);

        tracing::info!(
            "New track '{}' (alias '{}') is published.",
            full_track_namespace,
            track_alias
        );
        // The draft defines that the relay requires to send `PUBLISH_NAMESPACE` message to
        // any subscriber that has interests in the namespace
        // https://datatracker.ietf.org/doc/draft-ietf-moq-transport/

        // Convert DashMap<Namespace, DashSet<Uuid>> to DashMap<Uuid, DashSet<Namespace>>
        let combined = table.get_namespace_subscribers(&track_namespace);
        tracing::debug!("The namespace are subscribed by: {:?}", combined);
        for session_id in combined {
            if notifier
                .publish(
                    session_id,
                    track_namespace.clone(),
                    track_name.clone(),
                    track_alias,
                )
                .await
            {
                tracing::info!("Sent publish '{}' to {}", track_namespace, session_id);
            } else {
                tracing::warn!("Failed to send publish: {}", session_id);
            }
        }
    }

    async fn register_if_response_succeeded(
        &self,
        session_id: Uuid,
        table: &dyn Table,
        handler: Box<dyn PublishHandler>,
    ) {
        // TODO:
        // Send ok or error failed then close session.
        // forward: true case. prepare to accept stream/datagram before it returns the result.
        match handler.ok(128, FilterType::LatestObject).await {
            Ok(()) => table.register_publish(session_id, Arc::from(handler)).await,
            Err(_) => tracing::error!("failed to accept publish. close session."),
        }
    }
}
