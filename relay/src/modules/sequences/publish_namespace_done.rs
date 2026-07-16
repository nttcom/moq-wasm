use std::collections::HashSet;

use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    core::handler::publish_namespace_done::PublishNamespaceDoneHandler,
    inter_relay::InterRelayConnectionManager,
    route_registry::RelayRouteRegistry,
    sequences::{CascadingRelayContext, tables::table::LocalPubSubDirectory},
    types::SessionId,
};
use tracing::Span;

pub(crate) struct PublishNamespaceDone;

impl PublishNamespaceDone {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish_namespace_done",
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
        handler: &dyn PublishNamespaceDoneHandler,
    ) {
        let track_namespace = handler.track_namespace();
        tracing::info!(
            session_id = %session_id,
            track_namespace = %track_namespace,
            "SequenceHandler::PublishNamespaceDone"
        );

        let no_clients_remain = table.unregister_publish_namespace(session_id, track_namespace);

        Self::notify_local_subscribers(session_id, track_namespace, table, forwarder).await;

        if super::is_origin_client(session_id, forwarder).await && no_clients_remain {
            Self::withdraw_namespace_publication(
                track_namespace,
                forwarder,
                cascading_relay_context.route_registry,
                cascading_relay_context.inter_relay_connection_manager,
            )
            .await;
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish_namespace_done.notify_local_subscribers",
        skip_all,
        fields(track_namespace = %track_namespace)
    )]
    pub(crate) async fn notify_local_subscribers(
        publisher_session_id: SessionId,
        track_namespace: &str,
        table: &dyn LocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
    ) {
        for subscriber_session_id in table.get_namespace_subscribers(track_namespace) {
            if subscriber_session_id == publisher_session_id {
                continue;
            }
            if forwarder
                .publish_namespace_done(subscriber_session_id, track_namespace.to_string())
                .await
            {
                tracing::info!(
                    session_id = subscriber_session_id,
                    track_namespace = %track_namespace,
                    "forwarded PUBLISH_NAMESPACE_DONE"
                );
            } else {
                tracing::warn!(
                    session_id = subscriber_session_id,
                    track_namespace = %track_namespace,
                    "failed to forward PUBLISH_NAMESPACE_DONE"
                );
            }
        }
    }

    /// Unregisters the Redis publisher route and tells remote subscriber relays
    /// that the namespace has been withdrawn, so they can drop their local copy.
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.publish_namespace_done.withdraw_namespace_publication",
        skip_all,
        fields(track_namespace = %track_namespace)
    )]
    pub(crate) async fn withdraw_namespace_publication(
        track_namespace: &str,
        forwarder: &ControlMessageForwarder,
        route_registry: &dyn RelayRouteRegistry,
        inter_relay_connection_manager: &InterRelayConnectionManager,
    ) {
        if let Err(err) = route_registry
            .unregister_namespace_publisher(track_namespace)
            .await
        {
            tracing::warn!(
                ?err,
                track_namespace = %track_namespace,
                "failed to unregister namespace publisher route"
            );
        }

        let relays = match route_registry
            .find_namespace_subscribers(track_namespace)
            .await
        {
            Ok(relays) => relays,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    track_namespace = %track_namespace,
                    "failed to find remote namespace subscribers for PUBLISH_NAMESPACE_DONE"
                );
                return;
            }
        };

        let mut notified_relay_ids = HashSet::new();
        for relay in relays {
            if !notified_relay_ids.insert(relay.relay_id.clone()) {
                continue;
            }

            let session_id = match inter_relay_connection_manager.get_or_connect(&relay).await {
                Ok(session_id) => session_id,
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        relay_id = %relay.relay_id,
                        track_namespace = %track_namespace,
                        "failed to connect subscriber relay for PUBLISH_NAMESPACE_DONE"
                    );
                    continue;
                }
            };

            if !forwarder
                .publish_namespace_done(session_id, track_namespace.to_string())
                .await
            {
                tracing::warn!(
                    relay_id = %relay.relay_id,
                    session_id = session_id,
                    track_namespace = %track_namespace,
                    "failed to forward PUBLISH_NAMESPACE_DONE to subscriber relay"
                );
            }
        }
    }
}
