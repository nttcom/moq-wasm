use std::sync::Arc;

use tokio::sync::mpsc;

use crate::modules::{
    cascading::{
        inter_relay_connection_manager::InterRelayConnectionManager,
        route_registry::RelayRouteRegistry,
    },
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        sequences::{
            CascadingRelayContext,
            subscribe::upstream_join_task::{
                UpstreamJoin, UpstreamJoinDeps, UpstreamJoinTask, send_upstream_subscribes,
            },
        },
        upstream_publisher_resolver::{UpstreamPublisher, UpstreamPublisherResolver},
    },
    data_plane::ingress::ingress_coordinator::IngressCommand,
    domain::{
        error_code::PublishNamespaceErrorCode, pub_sub_directory::InMemoryLocalPubSubDirectory,
        session_id::SessionId, session_peer::SessionPeer,
    },
    session::handler::publish_namespace::PublishNamespaceHandler,
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
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &Arc<InMemoryLocalPubSubDirectory>,
        forwarder: &ControlMessageForwarder,
        ingress_sender: &mpsc::Sender<IngressCommand>,
        upstream_publisher_resolver: &Arc<UpstreamPublisherResolver>,
        cascading_relay_context: CascadingRelayContext<'_>,
        handler: &dyn PublishNamespaceHandler,
    ) {
        let track_namespace = handler.track_namespace();
        tracing::info!(
            session_id = %session_id,
            track_namespace = %track_namespace,
            "SequenceHandler::PublishNamespace"
        );

        let peer = super::session_peer(session_id, forwarder).await;
        let is_origin = peer == SessionPeer::Client;

        if is_origin
            && !self
                .register_route(
                    cascading_relay_context.route_registry,
                    track_namespace,
                    handler,
                )
                .await
        {
            return;
        }

        table.register_publish_namespace(session_id, track_namespace.to_string(), peer);

        if let Err(e) = handler.ok().await {
            tracing::error!("Publish Namespace Error: {:?}", e);
            return;
        }

        self.notify_to_subscribers(track_namespace, table, forwarder)
            .await;

        let join_deps = UpstreamJoinDeps {
            table: table.clone(),
            forwarder: forwarder.clone(),
            ingress_sender: ingress_sender.clone(),
        };
        if is_origin {
            Self::join_tracks_wanting_publishers(
                session_id,
                track_namespace,
                SessionPeer::Client,
                Some(session_id),
                vec![UpstreamPublisher::Session(session_id)],
                upstream_publisher_resolver,
                &join_deps,
            );
        } else {
            Self::join_tracks_wanting_publisher_relays(
                session_id,
                track_namespace,
                upstream_publisher_resolver,
                &join_deps,
                cascading_relay_context,
            )
            .await;
        }

        if is_origin {
            self.notify_remote_subscribers(
                track_namespace,
                forwarder,
                cascading_relay_context.route_registry,
                cascading_relay_context.inter_relay_connection_manager,
            )
            .await;
        }
    }

    /// A relay forwards a PUBLISH_NAMESPACE on whatever session it holds to
    /// this relay, so the publishing relays are looked up in the route
    /// registry and reached over the sessions this relay keeps per relay;
    /// subscribing over the announcing session could duplicate one of them.
    async fn join_tracks_wanting_publisher_relays(
        announcing_session_id: SessionId,
        track_namespace: &str,
        upstream_publisher_resolver: &Arc<UpstreamPublisherResolver>,
        join_deps: &UpstreamJoinDeps,
        cascading_relay_context: CascadingRelayContext<'_>,
    ) {
        let relays = match cascading_relay_context
            .route_registry
            .find_active_namespace_publishers(track_namespace)
            .await
        {
            Ok(relays) => relays,
            Err(err) => {
                tracing::warn!(?err, %track_namespace, "failed to find publisher relays to join");
                return;
            }
        };
        Self::join_tracks_wanting_publishers(
            announcing_session_id,
            track_namespace,
            SessionPeer::Relay,
            None,
            relays.into_iter().map(UpstreamPublisher::Relay).collect(),
            upstream_publisher_resolver,
            join_deps,
        );
    }

    /// draft-14 §8.4: a publisher announcing a namespace whose tracks other
    /// publishers already feed is subscribed to each of them.
    fn join_tracks_wanting_publishers(
        announcing_session_id: SessionId,
        track_namespace: &str,
        publisher_peer: SessionPeer,
        publisher_session_id: Option<SessionId>,
        publishers: Vec<UpstreamPublisher>,
        upstream_publisher_resolver: &Arc<UpstreamPublisherResolver>,
        join_deps: &UpstreamJoinDeps,
    ) {
        if publishers.is_empty() {
            return;
        }
        for track_key in join_deps.table.tracks_wanting_publisher(
            track_namespace,
            publisher_peer,
            publisher_session_id,
        ) {
            tracing::info!(%track_key, "subscribing newly announced publishers to an active track");
            let pending = send_upstream_subscribes(
                upstream_publisher_resolver,
                &join_deps.forwarder,
                &track_key,
                publishers.clone(),
            );
            let _upstream_join = UpstreamJoinTask::run(
                UpstreamJoin {
                    subscriber_session_id: announcing_session_id,
                    track_key,
                    pending,
                },
                join_deps.clone(),
            );
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
        table: &InMemoryLocalPubSubDirectory,
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
        let Err(err) = route_registry
            .register_namespace_publisher(track_namespace)
            .await
        else {
            return true;
        };
        tracing::warn!(?err, track_namespace = %track_namespace, "failed to register namespace route");
        if handler
            .error(
                PublishNamespaceErrorCode::InternalError as u64,
                "failed to register the namespace route".to_string(),
            )
            .await
            .is_err()
        {
            tracing::error!("failed to send `PUBLISH_NAMESPACE_ERROR`");
        }
        false
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
        for relay in routes {
            let Some(session_id) =
                super::connect_relay(inter_relay_connection_manager, &relay).await
            else {
                continue;
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::modules::{
        auth::verified_token::VerifiedToken,
        cascading::route_registry::NoopRelayRouteRegistry,
        domain::{pub_sub_directory::entry::UpstreamSubscriptionOrigin, track_key::TrackKey},
        session::session_repository::SessionRepository,
        test_support::{
            directory_fixtures::{
                PUBLISHER_SESSION, local_publisher_resolver, table_with_upstream,
            },
            mock_session::{
                MockPublishNamespaceHandler, recorded_session_answering_subscribe,
                session_repository_with_session,
            },
        },
    };

    const ANNOUNCING_PUBLISHER: SessionId = 3;

    async fn announce(table: &Arc<InMemoryLocalPubSubDirectory>) -> mpsc::Receiver<IngressCommand> {
        let (session, _recorded) = recorded_session_answering_subscribe();
        let repository = session_repository_with_session(
            ANNOUNCING_PUBLISHER,
            session,
            VerifiedToken::full_access(),
        )
        .await;
        let (session_event_sender, _session_event_receiver) = mpsc::unbounded_channel();
        let inter_relay_connection_manager = InterRelayConnectionManager::new(
            Arc::new(tokio::sync::Mutex::new(SessionRepository::new())),
            session_event_sender,
            "unused-relay-token".to_string(),
        );
        let (ingress_sender, ingress_receiver) = mpsc::channel(8);
        PublishNamespace
            .handle(
                ANNOUNCING_PUBLISHER,
                &Span::none(),
                table,
                &ControlMessageForwarder { repository },
                &ingress_sender,
                &Arc::new(local_publisher_resolver()),
                CascadingRelayContext {
                    route_registry: &NoopRelayRouteRegistry,
                    inter_relay_connection_manager: &inter_relay_connection_manager,
                },
                &MockPublishNamespaceHandler::new("ns"),
            )
            .await;
        ingress_receiver
    }

    #[tokio::test]
    async fn a_publisher_announcing_the_namespace_of_a_watched_track_is_subscribed_to_it() {
        // Arrange
        let (table, track_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        let table = Arc::new(table);
        table
            .register_downstream_subscription(2, 100, SessionPeer::Client, track_key.clone(), None)
            .unwrap();

        // Act
        let mut ingress_receiver = announce(&table).await;

        // Assert
        let started = tokio::time::timeout(Duration::from_secs(1), ingress_receiver.recv())
            .await
            .expect("the announcing publisher's ingress should start");
        assert!(matches!(
            started,
            Some(IngressCommand::Start(request)) if request.publisher_session_id == ANNOUNCING_PUBLISHER
        ));
        assert_eq!(
            table
                .get_upstream_track(&TrackKey::new("ns", "track"))
                .map(|track| track.subscriptions.into_keys().collect::<Vec<_>>()),
            Some(vec![PUBLISHER_SESSION, ANNOUNCING_PUBLISHER])
        );
    }

    #[tokio::test]
    async fn a_track_nobody_watches_is_not_subscribed_on_announcement() {
        // Arrange
        let (table, _) = table_with_upstream(UpstreamSubscriptionOrigin::Publish);
        let table = Arc::new(table);

        // Act
        let mut ingress_receiver = announce(&table).await;

        // Assert
        assert!(matches!(
            tokio::time::timeout(Duration::from_millis(100), ingress_receiver.recv()).await,
            Err(_) | Ok(None)
        ));
    }
}
