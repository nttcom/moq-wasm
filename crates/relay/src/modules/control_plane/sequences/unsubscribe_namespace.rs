use std::collections::HashSet;

use crate::modules::{
    cascading::{
        inter_relay_connection_manager::InterRelayConnectionManager,
        route_registry::{RelayInfo, RelayRouteRegistry},
    },
    control_plane::{
        control_message_forwarder::ControlMessageForwarder, sequences::CascadingRelayContext,
    },
    domain::{pub_sub_directory::InMemoryLocalPubSubDirectory, session_id::SessionId},
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
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        cascading_relay_context: CascadingRelayContext<'_>,
        track_namespace_prefix: &str,
    ) {
        tracing::info!(
            session_id = %session_id,
            track_namespace_prefix = %track_namespace_prefix,
            "SequenceHandler::UnsubscribeNamespace"
        );

        let no_clients_remain =
            table.unregister_subscribe_namespace(session_id, track_namespace_prefix);
        if !super::is_origin_client(session_id, forwarder).await {
            return;
        }
        if !no_clients_remain {
            return;
        }

        Self::cleanup_empty_namespace_subscription(
            track_namespace_prefix,
            table,
            forwarder,
            cascading_relay_context.route_registry,
            cascading_relay_context.inter_relay_connection_manager,
        )
        .await;
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.unsubscribe_namespace.cleanup_empty_namespace_subscription",
        skip_all,
        fields(track_namespace_prefix = %track_namespace_prefix)
    )]
    pub(crate) async fn cleanup_empty_namespace_subscription(
        track_namespace_prefix: &str,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        route_registry: &dyn RelayRouteRegistry,
        inter_relay_connection_manager: &InterRelayConnectionManager,
    ) {
        table.purge_relay_publish_namespaces(track_namespace_prefix);

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
            let Some(session_id) =
                super::connect_relay(inter_relay_connection_manager, &route).await
            else {
                continue;
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
            let publisher_relays = match route_registry
                .find_active_namespace_publishers(&namespace_route.track_namespace)
                .await
            {
                Ok(publisher_relays) => publisher_relays,
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        track_namespace = %namespace_route.track_namespace,
                        "failed to find publisher relays for UNSUBSCRIBE_NAMESPACE"
                    );
                    continue;
                }
            };

            for relay in publisher_relays {
                if relay_ids.insert(relay.relay_id.clone()) {
                    relays.push(relay);
                }
            }
        }

        relays
    }
}
