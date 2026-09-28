use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    core::handler::subscribe_namespace::SubscribeNamespaceHandler,
    enums::SubscribeNamespaceErrorCode,
    route_registry::{RegisterRouteError, RelayRouteRegistry},
    sequences::tables::{hashmap_table::InMemoryLocalPubSubDirectory, table::PeerKind},
    types::SessionId,
};
use tracing::Span;

pub(crate) struct SubscribeNameSpace;

impl SubscribeNameSpace {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe_namespace",
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
        route_registry: &dyn RelayRouteRegistry,
        handler: &dyn SubscribeNamespaceHandler,
    ) {
        let track_namespace_prefix = handler.track_namespace_prefix();
        tracing::info!(
            session_id = %session_id,
            track_namespace_prefix = %track_namespace_prefix,
            "SequenceHandler::SubscribeNamespace"
        );
        let peer_kind = if super::is_origin_client(session_id, forwarder).await {
            PeerKind::Client
        } else {
            PeerKind::Relay
        };
        let is_first_client = table.register_subscribe_namespace(
            session_id,
            track_namespace_prefix.to_string(),
            peer_kind,
        );
        if is_first_client
            && !self
                .register_route(route_registry, track_namespace_prefix, handler)
                .await
        {
            table.unregister_subscribe_namespace(session_id, track_namespace_prefix);
            return;
        }
        self.broadcast_to_subscribers(session_id, track_namespace_prefix, forwarder, table)
            .await;
        self.notify_remote_publish_namespaces(
            session_id,
            track_namespace_prefix,
            forwarder,
            route_registry,
        )
        .await;
        if let Err(e) = handler.ok().await {
            tracing::error!("Subscribe Namespace Error: {:?}", e);
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe_namespace.register_route",
        skip_all,
        fields(track_namespace_prefix = %track_namespace_prefix)
    )]
    async fn register_route(
        &self,
        route_registry: &dyn RelayRouteRegistry,
        track_namespace_prefix: &str,
        handler: &dyn SubscribeNamespaceHandler,
    ) -> bool {
        match route_registry
            .register_namespace_subscriber(track_namespace_prefix)
            .await
        {
            Ok(()) => true,
            Err(RegisterRouteError::Conflict) => {
                tracing::warn!(track_namespace_prefix = %track_namespace_prefix, "namespace already has an active subscriber");
                match handler
                    .error(
                        SubscribeNamespaceErrorCode::NamespacePrefixOverlap as u64,
                        "namespace already subscribed".to_string(),
                    )
                    .await
                {
                    Ok(_) => tracing::info!("sent `SUBSCRIBE_NAMESPACE_ERROR` ok"),
                    Err(_) => tracing::error!("failed to send `SUBSCRIBE_NAMESPACE_ERROR`"),
                }
                false
            }
            Err(err) => {
                tracing::warn!(?err, track_namespace_prefix = %track_namespace_prefix, "failed to register namespace subscription route");
                false
            }
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe_namespace.broadcast_to_subscribers",
        skip_all,
        fields(session_id = %session_id, track_namespace_prefix = %track_namespace_prefix)
    )]
    async fn broadcast_to_subscribers(
        &self,
        session_id: SessionId,
        track_namespace_prefix: &str,
        forwarder: &ControlMessageForwarder,
        table: &InMemoryLocalPubSubDirectory,
    ) {
        let filtered = table.get_subscribers(track_namespace_prefix);
        for (track_namespace, published_track) in filtered {
            if let Some((track_name, _)) = published_track {
                if forwarder
                    .publish(session_id, track_namespace.clone(), track_name.clone())
                    .await
                {
                    tracing::info!(
                        "Forwarded PUBLISH '{}' to session:{}",
                        track_namespace,
                        session_id
                    )
                } else {
                    tracing::warn!("Failed to forward PUBLISH: {}", session_id);
                }
            } else if forwarder
                .publish_namespace(session_id, track_namespace.clone())
                .await
            {
                tracing::info!(
                    "Forwarded PUBLISH_NAMESPACE '{}' to {}",
                    track_namespace,
                    session_id
                );
            } else {
                tracing::error!("Failed to forward PUBLISH_NAMESPACE");
            }
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe_namespace.notify_remote_publish_namespaces",
        skip_all,
        fields(session_id = %session_id, track_namespace_prefix = %track_namespace_prefix)
    )]
    async fn notify_remote_publish_namespaces(
        &self,
        session_id: SessionId,
        track_namespace_prefix: &str,
        forwarder: &ControlMessageForwarder,
        route_registry: &dyn RelayRouteRegistry,
    ) {
        let routes = match route_registry
            .find_namespace_publishers_by_prefix(track_namespace_prefix)
            .await
        {
            Ok(routes) => routes,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    session_id = session_id,
                    track_namespace_prefix = %track_namespace_prefix,
                    "failed to find remote namespace routes"
                );
                return;
            }
        };

        for route in routes {
            if forwarder
                .publish_namespace(session_id, route.track_namespace.clone())
                .await
            {
                tracing::info!(
                    session_id = session_id,
                    track_namespace = %route.track_namespace,
                    "forwarded remote PUBLISH_NAMESPACE"
                );
            } else {
                tracing::warn!(
                    session_id = session_id,
                    track_namespace = %route.track_namespace,
                    "failed to forward remote PUBLISH_NAMESPACE"
                );
            }
        }
    }
}
