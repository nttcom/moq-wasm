use std::{collections::HashSet, sync::Arc};

use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    core::handler::publish::PublishHandler,
    enums::FilterType,
    inter_relay::InterRelayConnectionManager,
    route_registry::{RelayRouteRegistry, RouteStatus},
    sequences::{CascadingRelayContext, tables::table::LocalPubSubDirectory},
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
        table: &dyn LocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        cascading_relay_context: CascadingRelayContext<'_>,
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
        self.broadcast_to_subscribers(session_id, forwarder, table, handler.as_ref())
            .await;
        if self
            .register_if_response_succeeded(session_id, table, handler)
            .await
            && self.is_origin_client(session_id, forwarder).await
        {
            self.register_route(
                cascading_relay_context.route_registry,
                &track_namespace,
                &track_name,
            )
            .await;
            self.notify_remote_subscribers(
                &track_namespace,
                &track_name,
                forwarder,
                cascading_relay_context.route_registry,
                cascading_relay_context.inter_relay_connection_manager,
            )
            .await;
        }
        tracing::info!(
            session_id = %session_id,
            track_namespace = %track_namespace,
            track_name = %track_name,
            track_alias = %track_alias,
            "SequenceHandler::publish DONE"
        );
    }

    async fn is_origin_client(
        &self,
        session_id: SessionId,
        forwarder: &ControlMessageForwarder,
    ) -> bool {
        forwarder
            .repository
            .lock()
            .await
            .is_client_session(session_id)
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
        forwarder: &ControlMessageForwarder,
        table: &dyn LocalPubSubDirectory,
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
            if let Some(subscriber_track_alias) = forwarder
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
        table: &dyn LocalPubSubDirectory,
        handler: Box<dyn PublishHandler>,
    ) -> bool {
        // TODO:
        // Send ok or error failed then close session.
        // forward: true case. prepare to accept stream/datagram before it returns the result.
        match handler.ok(128, FilterType::LargestObject, 0).await {
            Ok(()) => {
                table.register_publish(session_id, Arc::from(handler)).await;
                true
            }
            Err(_) => {
                tracing::error!("failed to accept publish. close session.");
                false
            }
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish.register_route",
        skip_all,
        fields(track_namespace = %track_namespace, track_name = %track_name)
    )]
    async fn register_route(
        &self,
        route_registry: &dyn RelayRouteRegistry,
        track_namespace: &str,
        track_name: &str,
    ) {
        if let Err(err) = route_registry
            .register_track_route(track_namespace, track_name, RouteStatus::Active)
            .await
        {
            tracing::warn!(
                ?err,
                track_namespace = %track_namespace,
                track_name = %track_name,
                "failed to register track route"
            );
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish.notify_remote_subscribers",
        skip_all,
        fields(track_namespace = %track_namespace, track_name = %track_name)
    )]
    async fn notify_remote_subscribers(
        &self,
        track_namespace: &str,
        track_name: &str,
        forwarder: &ControlMessageForwarder,
        route_registry: &dyn RelayRouteRegistry,
        inter_relay_connection_manager: &InterRelayConnectionManager,
    ) {
        let routes = match route_registry
            .find_active_namespace_subscribers(track_namespace)
            .await
        {
            Ok(routes) => routes,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    track_namespace = %track_namespace,
                    track_name = %track_name,
                    "failed to find remote publish subscribers"
                );
                return;
            }
        };
        let publisher_relay_ids = match route_registry
            .find_active_track_routes(track_namespace, track_name)
            .await
        {
            Ok(routes) => routes
                .into_iter()
                .map(|route| route.relay.relay_id)
                .collect::<HashSet<_>>(),
            Err(err) => {
                tracing::warn!(
                    ?err,
                    track_namespace = %track_namespace,
                    track_name = %track_name,
                    "failed to find track publisher routes"
                );
                HashSet::new()
            }
        };

        for route in routes {
            if publisher_relay_ids.contains(&route.relay.relay_id) {
                continue;
            }

            let session_id = match inter_relay_connection_manager
                .get_or_connect(&route.relay)
                .await
            {
                Ok(session_id) => session_id,
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        relay_id = %route.relay.relay_id,
                        track_namespace = %track_namespace,
                        track_name = %track_name,
                        "failed to connect remote publish subscriber"
                    );
                    continue;
                }
            };

            if forwarder
                .publish(
                    session_id,
                    track_namespace.to_string(),
                    track_name.to_string(),
                )
                .await
                .is_some()
            {
                tracing::info!(
                    relay_id = %route.relay.relay_id,
                    session_id = session_id,
                    track_namespace = %track_namespace,
                    track_name = %track_name,
                    "forwarded PUBLISH to remote relay"
                );
            } else {
                tracing::warn!(
                    relay_id = %route.relay.relay_id,
                    session_id = session_id,
                    track_namespace = %track_namespace,
                    track_name = %track_name,
                    "failed to forward PUBLISH to remote relay"
                );
            }
        }
    }
}
