use moqt::RequestErrorCode;
use std::sync::Arc;

use crate::modules::{
    cascading::{
        inter_relay_connection_manager::InterRelayConnectionManager,
        route_registry::RelayRouteRegistry,
    },
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        sequences::{
            CascadingRelayContext,
            subscribe::pending_producer_task::{PendingProducerDeps, subscribe_publishers},
        },
        upstream_publisher_resolver::{UpstreamPublisher, UpstreamPublisherResolver},
    },
    data_plane::cache::store::TrackCacheStore,
    directory::InMemoryLocalPubSubDirectory,
    domain::{session_id::SessionId, session_peer::SessionPeer},
    session::handler::publish_namespace::PublishNamespaceHandler,
};

pub(crate) struct PublishNamespace;

impl PublishNamespace {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        table: &Arc<InMemoryLocalPubSubDirectory>,
        forwarder: &ControlMessageForwarder,
        cache_store: &Arc<TrackCacheStore>,
        upstream_publisher_resolver: &Arc<UpstreamPublisherResolver>,
        cascading_relay_context: CascadingRelayContext<'_>,
        handler: &dyn PublishNamespaceHandler,
    ) {
        let track_namespace = handler.track_namespace();
        tracing::info!(
            session_id = %session_id,
            track_namespace = %track_namespace,
            "PUBLISH_NAMESPACE received"
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
            tracing::error!(error = ?e, "PUBLISH_NAMESPACE failed");
            return;
        }

        self.notify_to_subscribers(track_namespace, table, forwarder)
            .await;

        let join_deps = PendingProducerDeps {
            table: table.clone(),
            forwarder: forwarder.clone(),
            cache_store: cache_store.clone(),
            resolver: upstream_publisher_resolver.clone(),
        };
        if is_origin {
            Self::join_tracks_wanting_publishers(
                session_id,
                track_namespace,
                SessionPeer::Client,
                Some(session_id),
                vec![UpstreamPublisher::Session(session_id)],
                &join_deps,
            );
            self.notify_remote_subscribers(
                track_namespace,
                forwarder,
                cascading_relay_context.route_registry,
                cascading_relay_context.inter_relay_connection_manager,
            )
            .await;
        } else {
            Self::join_tracks_wanting_publisher_relays(
                session_id,
                track_namespace,
                &join_deps,
                cascading_relay_context,
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
        join_deps: &PendingProducerDeps,
        cascading_relay_context: CascadingRelayContext<'_>,
    ) {
        let relays = match cascading_relay_context
            .route_registry
            .find_active_namespace_publishers(track_namespace)
            .await
        {
            Ok(relays) => relays,
            Err(err) => {
                tracing::warn!(error = ?err, %track_namespace, "failed to find publisher relays to join");
                return;
            }
        };
        Self::join_tracks_wanting_publishers(
            announcing_session_id,
            track_namespace,
            SessionPeer::Relay,
            None,
            relays.into_iter().map(UpstreamPublisher::Relay).collect(),
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
        join_deps: &PendingProducerDeps,
    ) {
        if publishers.is_empty() {
            return;
        }
        for track_key in join_deps.table.tracks_wanting_publisher(
            track_namespace,
            publisher_peer,
            publisher_session_id,
        ) {
            tracing::debug!(
                track_namespace = %track_key.track_namespace,
                track_name = %track_key.track_name,
                "subscribing newly announced publishers to an active track"
            );
            subscribe_publishers(
                join_deps,
                &track_key,
                announcing_session_id,
                publishers.clone(),
            );
        }
    }

    #[tracing::instrument(
        level = "debug",
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
            forwarder
                .publish_namespace(session_id, track_namespace.to_string())
                .await;
        }
    }

    #[tracing::instrument(
        level = "debug",
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
        tracing::warn!(error = ?err, track_namespace = %track_namespace, "failed to register namespace route");
        if handler
            .error(
                RequestErrorCode::InternalError,
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
        level = "debug",
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
                    error = ?err,
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

            forwarder
                .publish_namespace(session_id, track_namespace.to_string())
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::modules::test_support::directory_fixtures::delivery_control;
    use crate::modules::test_support::directory_fixtures::downstream_subscription;
    use crate::modules::test_support::directory_fixtures::unused_inter_relay_client_config;
    use std::time::Duration;

    use super::*;
    use crate::modules::{
        auth::verified_token::VerifiedToken,
        cascading::route_registry::NoopRelayRouteRegistry,
        directory::track::ProducerOrigin,
        domain::track_key::TrackKey,
        test_support::{
            directory_fixtures::{
                PUBLISHER_SESSION, local_publisher_resolver, table_with_upstream,
            },
            mock_session::{
                MockPublishNamespaceHandler, RecordedControlMessages, detached_session_repository,
                recorded_session_answering_subscribe, session_repository_with_session, wait_until,
            },
        },
    };

    const ANNOUNCING_PUBLISHER: SessionId = 3;

    const ANNOUNCING_UPSTREAM_REQUEST_ID: u64 = 1;

    async fn announce(table: &Arc<InMemoryLocalPubSubDirectory>) -> RecordedControlMessages {
        let (session, recorded) =
            recorded_session_answering_subscribe(|| moqt::object::ContentExists::False);
        let repository = session_repository_with_session(
            ANNOUNCING_PUBLISHER,
            session,
            VerifiedToken::full_access(),
        )
        .await;
        let inter_relay_connection_manager = InterRelayConnectionManager::new(
            Arc::new(tokio::sync::Mutex::new(detached_session_repository())),
            unused_inter_relay_client_config(),
        );
        PublishNamespace
            .handle(
                ANNOUNCING_PUBLISHER,
                table,
                &ControlMessageForwarder { repository },
                &Arc::new(TrackCacheStore::new()),
                &Arc::new(local_publisher_resolver()),
                CascadingRelayContext {
                    route_registry: &NoopRelayRouteRegistry,
                    inter_relay_connection_manager: &inter_relay_connection_manager,
                },
                &MockPublishNamespaceHandler::new("ns"),
            )
            .await;
        recorded
    }

    #[tokio::test]
    async fn a_publisher_announcing_the_namespace_of_a_watched_track_is_subscribed_to_it() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);
        let table = Arc::new(table);
        table
            .register_downstream_subscription(
                2,
                100,
                SessionPeer::Client,
                downstream_subscription(track_key.clone(), None),
                delivery_control(true),
            )
            .unwrap();

        // Act
        let recorded = announce(&table).await;

        // Assert
        recorded
            .wait_ingest_started(ANNOUNCING_UPSTREAM_REQUEST_ID)
            .await;
        let publishers = || {
            table
                .track_state(&TrackKey::new("ns", "track"))
                .map(|track| track.producers.into_keys().collect::<Vec<_>>())
        };
        wait_until(|| publishers() == Some(vec![PUBLISHER_SESSION, ANNOUNCING_PUBLISHER]))
            .await
            .expect("the announcing publisher should join the track");
    }

    #[tokio::test]
    async fn a_track_nobody_watches_is_not_subscribed_on_announcement() {
        // Arrange
        let (table, _) = table_with_upstream(ProducerOrigin::Publish);
        let table = Arc::new(table);

        // Act
        let recorded = announce(&table).await;

        // Assert
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(recorded.data_receivers().is_empty());
    }
}
