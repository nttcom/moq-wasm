use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    core::handler::publish_namespace::PublishNamespaceHandler,
    inter_relay::InterRelayConnectionManager,
    route_registry::{RegisterNamespacePublisherError, RelayRouteRegistry, RouteStatus},
    sequences::{CascadingRelayContext, tables::table::LocalPubSubDirectory},
    types::SessionId,
};
use tracing::Span;

pub(crate) struct PublishNamespace;

impl PublishNamespace {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish_namespace",
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
        handler: &dyn PublishNamespaceHandler,
    ) {
        let requested_track_namespace = handler.track_namespace();
        tracing::info!(
            session_id = %session_id,
            track_namespace = %requested_track_namespace,
            "SequenceHandler::PublishNamespace"
        );

        let is_origin = self.is_origin_client(session_id, forwarder).await;

        if is_origin
            && !self
                .register_route(
                    cascading_relay_context.route_registry,
                    requested_track_namespace,
                    handler,
                )
                .await
        {
            return;
        }

        let Some(track_namespace) = self.register(session_id, table, handler).await else {
            return;
        };

        if !self.response(handler).await {
            return;
        }

        // The draft defines that the relay requires to send `PUBLISH_NAMESPACE` message to
        // any subscriber that has interests in the namespace
        // https://datatracker.ietf.org/doc/draft-ietf-moq-transport/

        // Convert DashMap<Namespace, DashSet<SessionId>> to DashMap<SessionId, DashSet<Namespace>>
        self.notify_to_subscribers(&track_namespace, table, forwarder)
            .await;

        if is_origin {
            self.notify_remote_subscribers(
                &track_namespace,
                forwarder,
                cascading_relay_context.route_registry,
                cascading_relay_context.inter_relay_connection_manager,
            )
            .await;
        }
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
        name = "relay.sequence.publish_namespace.register",
        skip_all,
        fields(session_id = %session_id, track_namespace = %handler.track_namespace())
    )]
    async fn register(
        &self,
        session_id: SessionId,
        table: &dyn LocalPubSubDirectory,
        handler: &dyn PublishNamespaceHandler,
    ) -> Option<String> {
        let track_namespace = handler.track_namespace();

        if !table.register_publish_namespace(session_id, track_namespace.to_string()) {
            // TODO: Session close.
            tracing::error!("Failed to register publish namespace");
            match handler
                .error(0, "Failed to register publish namespace".to_string())
                .await
            {
                Ok(_) => tracing::info!("send `PUBLISH_NAMESPACE_ERROR` ok"),
                Err(_) => {
                    tracing::error!("Failed to send `PUBLISH_NAMESPACE_ERROR`. Session close.")
                }
            }
            None
        } else {
            Some(track_namespace.to_string())
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish_namespace.notify_to_subscribers",
        skip_all,
        fields(track_namespace = %track_namespace)
    )]
    async fn notify_to_subscribers(
        &self,
        track_namespace: &str,
        table: &dyn LocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
    ) {
        let combined = table.get_namespace_subscribers(track_namespace);
        tracing::debug!("The namespace are subscribed by: {:?}", combined);
        for session_id in combined {
            if forwarder
                .publish_namespace(session_id, track_namespace.to_string())
                .await
            {
                tracing::info!(
                    "Sent publish namespace '{}' to {}",
                    track_namespace,
                    session_id
                )
            } else {
                tracing::warn!("Failed to send publish namespace: {}", session_id);
            }
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish_namespace.response",
        skip_all
    )]
    async fn response(&self, handler: &dyn PublishNamespaceHandler) -> bool {
        match handler.ok().await {
            Ok(_) => true,
            Err(e) => {
                tracing::error!("Publish Namespace Error: {:?}", e);
                false
            }
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish_namespace.register_route",
        skip_all,
        fields(track_namespace = %track_namespace)
    )]
    async fn register_route(
        &self,
        route_registry: &dyn RelayRouteRegistry,
        track_namespace: &str,
        handler: &dyn PublishNamespaceHandler,
    ) -> bool {
        match route_registry
            .register_namespace_publisher(track_namespace, RouteStatus::Active)
            .await
        {
            Ok(()) => true,
            Err(RegisterNamespacePublisherError::Conflict) => {
                tracing::warn!(track_namespace = %track_namespace, "namespace already has an active publisher");
                match handler.error(0, "namespace already published".to_string()).await {
                    Ok(_) => tracing::info!("sent `PUBLISH_NAMESPACE_ERROR` ok"),
                    Err(_) => tracing::error!("failed to send `PUBLISH_NAMESPACE_ERROR`"),
                }
                false
            }
            Err(err) => {
                tracing::warn!(?err, track_namespace = %track_namespace, "failed to register namespace route");
                false
            }
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish_namespace.notify_remote_subscribers",
        skip_all,
        fields(track_namespace = %track_namespace)
    )]
    async fn notify_remote_subscribers(
        &self,
        track_namespace: &str,
        forwarder: &ControlMessageForwarder,
        route_registry: &dyn RelayRouteRegistry,
        inter_relay_connection_manager: &InterRelayConnectionManager,
    ) {
        let routes = match route_registry
            .find_namespace_subscribers(track_namespace)
            .await
        {
            Ok(routes) => routes,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    track_namespace = %track_namespace,
                    "failed to find remote namespace subscribers"
                );
                return;
            }
        };
        let publisher_relay_id = match route_registry
            .find_active_namespace_publisher(track_namespace)
            .await
        {
            Ok(relay) => relay.map(|r| r.relay_id),
            Err(err) => {
                tracing::warn!(
                    ?err,
                    track_namespace = %track_namespace,
                    "failed to find namespace publisher routes"
                );
                None
            }
        };

        for relay in routes {
            if publisher_relay_id.as_deref() == Some(relay.relay_id.as_str()) {
                continue;
            }

            let session_id = match inter_relay_connection_manager
                .get_or_connect(&relay)
                .await
            {
                Ok(session_id) => session_id,
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        relay_id = %relay.relay_id,
                        track_namespace = %track_namespace,
                        "failed to connect remote namespace subscriber"
                    );
                    continue;
                }
            };

            if forwarder
                .publish_namespace(session_id, track_namespace.to_string())
                .await
            {
                tracing::info!(
                    relay_id = %relay.relay_id,
                    session_id = session_id,
                    track_namespace = %track_namespace,
                    "forwarded PUBLISH_NAMESPACE to remote relay"
                );
            } else {
                tracing::warn!(
                    relay_id = %relay.relay_id,
                    session_id = session_id,
                    track_namespace = %track_namespace,
                    "failed to forward PUBLISH_NAMESPACE to remote relay"
                );
            }
        }
    }
}
