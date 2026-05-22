use std::sync::Arc;

use crate::modules::{
    core::handler::publish::PublishHandler,
    enums::FilterType,
    sequences::{notifier::SessionSignalingDispatcher, tables::table::SignalingStateTable},
    types::SessionId,
};
use tracing::Span;

pub(crate) struct Publish;

impl Publish {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish",
        skip_all,
        parent = session_span,
        fields(session_id = %session_id)
    )]
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &dyn SignalingStateTable,
        notifier: &SessionSignalingDispatcher,
        handler: Box<dyn PublishHandler>,
    ) {
        let track_namespace = handler.track_namespace().to_string();
        let track_name = handler.track_name().to_string();
        let track_alias = handler.track_alias();
        tracing::info!(
            session_id = %session_id,
            track_namespace = %track_namespace,
            track_name = %track_name,
            track_alias = %track_alias,
            "SequenceHandler::publish"
        );
        self.broadcast_to_subscribers(session_id, notifier, table, handler.as_ref())
            .await;
        self.register_if_response_succeeded(session_id, table, handler)
            .await;
        tracing::info!(
            session_id = %session_id,
            track_namespace = %track_namespace,
            track_name = %track_name,
            track_alias = %track_alias,
            "SequenceHandler::publish DONE"
        );
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish.broadcast_to_subscribers",
        skip_all,
        fields(publisher_session_id = %publisher_session_id, track_namespace = %handler.track_namespace(), track_name = %handler.track_name())
    )]
    async fn broadcast_to_subscribers(
        &self,
        publisher_session_id: SessionId,
        notifier: &SessionSignalingDispatcher,
        table: &dyn SignalingStateTable,
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

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish.register_if_response_succeeded",
        skip_all,
        fields(session_id = %session_id)
    )]
    async fn register_if_response_succeeded(
        &self,
        session_id: SessionId,
        table: &dyn SignalingStateTable,
        handler: Box<dyn PublishHandler>,
    ) {
        // TODO:
        // Send ok or error failed then close session.
        // forward: true case. prepare to accept stream/datagram before it returns the result.
        match handler.ok(128, FilterType::LargestObject, 0).await {
            Ok(()) => table.register_publish(session_id, Arc::from(handler)).await,
            Err(_) => tracing::error!("failed to accept publish. close session."),
        }
    }
}
