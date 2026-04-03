use std::sync::Arc;

use crate::modules::{
    core::handler::publish::PublishHandler,
    enums::FilterType,
    sequences::{notifier::Notifier, tables::table::Table},
    types::SessionId,
};

pub(crate) struct Publish;

impl Publish {
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        table: &dyn Table,
        notifier: &Notifier,
        handler: Box<dyn PublishHandler>,
    ) {
        tracing::info!("SequenceHandler::publish: {}", session_id);
        self.broadcast_to_subscribers(session_id, notifier, table, handler.as_ref())
            .await;
        self.register_if_response_succeeded(session_id, table, handler)
            .await;
        tracing::info!("SequenceHandler::publish: {} DONE", session_id);
    }

    async fn broadcast_to_subscribers(
        &self,
        publisher_session_id: SessionId,
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

        // Convert DashMap<Namespace, DashSet<SessionId>> to DashMap<SessionId, DashSet<Namespace>>
        let combined = table.get_namespace_subscribers(&track_namespace);
        tracing::debug!("The namespace are subscribed by: {:?}", combined);
        for subscriber_session_id in combined {
            if let Some(subscriber_track_alias) = notifier
                .publish(
                    subscriber_session_id,
                    track_namespace.clone(),
                    track_name.clone(),
                )
                .await
            {
                table.register_track_alias_link(
                    publisher_session_id,
                    track_alias,
                    subscriber_session_id,
                    subscriber_track_alias,
                );
                tracing::info!(
                    "Sent publish '{}' to {}",
                    track_namespace,
                    subscriber_session_id
                );
            } else {
                tracing::warn!("Failed to send publish: {}", subscriber_session_id);
            }
        }
    }

    async fn register_if_response_succeeded(
        &self,
        session_id: SessionId,
        table: &dyn Table,
        handler: Box<dyn PublishHandler>,
    ) {
        // TODO:
        // Send ok or error failed then close session.
        // forward: true case. prepare to accept stream/datagram before it returns the result.
        match handler.ok(128, FilterType::LargestObject).await {
            Ok(()) => table.register_publish(session_id, Arc::from(handler)).await,
            Err(_) => tracing::error!("failed to accept publish. close session."),
        }
    }
}
