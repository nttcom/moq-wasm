use std::collections::HashSet;

use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    core::handler::unsubscribe_namespace::UnsubscribeNamespaceHandler,
    inter_relay::InterRelayConnectionManager,
    route_registry::{RelayInfo, RelayRouteRegistry},
    sequences::{CascadingRelayContext, tables::table::LocalPubSubDirectory},
    types::SessionId,
};
use tracing::Span;

pub(crate) struct UnsubscribeNamespace;

impl UnsubscribeNamespace {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.unsubscribe_namespace",
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
        handler: &dyn UnsubscribeNamespaceHandler,
    ) {
        let track_namespace_prefix = handler.track_namespace_prefix();
        tracing::info!(
            session_id = %session_id,
            track_namespace_prefix = %track_namespace_prefix,
            "SequenceHandler::UnsubscribeNamespace"
        );

        let is_empty = table.unregister_subscribe_namespace(session_id, track_namespace_prefix);
        if !is_empty || !Self::is_origin_client(session_id, forwarder).await {
            return;
        }

        Self::cleanup_empty_namespace_subscription(
            track_namespace_prefix,
            forwarder,
            cascading_relay_context.route_registry,
            cascading_relay_context.inter_relay_connection_manager,
        )
        .await;
    }

    async fn is_origin_client(session_id: SessionId, forwarder: &ControlMessageForwarder) -> bool {
        forwarder
            .repository
            .lock()
            .await
            .is_client_session(session_id)
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.unsubscribe_namespace.cleanup_empty_namespace_subscription",
        skip_all,
        fields(track_namespace_prefix = %track_namespace_prefix)
    )]
    pub(crate) async fn cleanup_empty_namespace_subscription(
        track_namespace_prefix: &str,
        forwarder: &ControlMessageForwarder,
        route_registry: &dyn RelayRouteRegistry,
        inter_relay_connection_manager: &InterRelayConnectionManager,
    ) {
        if let Err(err) = route_registry
            .unregister_namespace_subscriber(track_namespace_prefix)
            .await
        {
            tracing::warn!(
                ?err,
                track_namespace_prefix = %track_namespace_prefix,
                "failed to unregister namespace subscription route"
            );
        }

        let routes = Self::find_publisher_relays(track_namespace_prefix, route_registry).await;
        for route in routes {
            let session_id = match inter_relay_connection_manager
                .get_or_connect(&route)
                .await
            {
                Ok(session_id) => session_id,
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        relay_id = %route.relay_id,
                        track_namespace_prefix = %track_namespace_prefix,
                        "failed to connect upstream relay for UNSUBSCRIBE_NAMESPACE"
                    );
                    continue;
                }
            };

            if let Err(err) = forwarder
                .unsubscribe_namespace(session_id, track_namespace_prefix.to_string())
                .await
            {
                tracing::warn!(
                    ?err,
                    relay_id = %route.relay_id,
                    session_id = session_id,
                    track_namespace_prefix = %track_namespace_prefix,
                    "failed to forward UNSUBSCRIBE_NAMESPACE to upstream relay"
                );
            }
        }
    }

    async fn find_publisher_relays(
        track_namespace_prefix: &str,
        route_registry: &dyn RelayRouteRegistry,
    ) -> Vec<RelayInfo> {
        let namespace_routes = match route_registry
            .find_namespace_publishers_by_prefix(track_namespace_prefix)
            .await
        {
            Ok(routes) => routes,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    track_namespace_prefix = %track_namespace_prefix,
                    "failed to find namespace routes for UNSUBSCRIBE_NAMESPACE"
                );
                return Vec::new();
            }
        };

        let mut relay_ids = HashSet::new();
        let mut relays = Vec::new();
        for namespace_route in namespace_routes {
            let relay = match route_registry
                .find_active_namespace_publisher(&namespace_route.track_namespace)
                .await
            {
                Ok(relay) => relay,
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        track_namespace = %namespace_route.track_namespace,
                        "failed to find publisher relays for UNSUBSCRIBE_NAMESPACE"
                    );
                    continue;
                }
            };

            if let Some(relay) = relay {
                if relay_ids.insert(relay.relay_id.clone()) {
                    relays.push(relay);
                }
            }
        }

        relays
    }
}
