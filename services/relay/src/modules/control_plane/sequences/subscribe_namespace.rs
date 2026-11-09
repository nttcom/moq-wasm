use crate::modules::{
    cascading::route_registry::{RegisterRouteError, RelayRouteRegistry},
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        sequences::downstream_publish::{
            DownstreamPublish, downstream_publish_task::DownstreamPublishTask,
        },
    },
    directory::{
        InMemoryLocalPubSubDirectory,
        track::{MatchingPublication, NamespaceSubscription},
    },
    domain::session_id::SessionId,
    session::handler::subscribe_namespace::SubscribeNamespaceHandler,
};
use std::collections::HashSet;

use moqt::RequestErrorCode;

pub(crate) struct SubscribeNameSpace;

impl SubscribeNameSpace {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        downstream_publish: &DownstreamPublish,
        route_registry: &dyn RelayRouteRegistry,
        handler: &dyn SubscribeNamespaceHandler,
    ) {
        let track_namespace_prefix = handler.track_namespace_prefix();
        let subscription = NamespaceSubscription {
            interest: handler.interest(),
            peer: super::session_peer(session_id, forwarder).await,
            forward: handler.forward(),
        };
        tracing::info!(
            session_id = %session_id,
            track_namespace_prefix = %track_namespace_prefix,
            ?subscription,
            "SUBSCRIBE_NAMESPACE received"
        );
        if table.has_overlapping_namespace_subscription(
            session_id,
            track_namespace_prefix,
            subscription.interest,
        ) {
            tracing::warn!(
                session_id = %session_id,
                track_namespace_prefix = %track_namespace_prefix,
                "SUBSCRIBE_NAMESPACE overlaps an active one of the session"
            );
            if let Err(error) = handler
                .error(
                    RequestErrorCode::NamespacePrefixOverlap,
                    "the prefix overlaps an active SUBSCRIBE_NAMESPACE".to_string(),
                )
                .await
            {
                tracing::warn!(%error, session_id = %session_id, "failed to send SUBSCRIBE_NAMESPACE_ERROR");
            }
            return;
        }
        let is_first_client = table.register_subscribe_namespace(
            session_id,
            track_namespace_prefix.to_string(),
            subscription,
        );
        if is_first_client
            && !self
                .register_route(route_registry, track_namespace_prefix, handler)
                .await
        {
            table.unregister_subscribe_namespace(
                session_id,
                track_namespace_prefix,
                subscription.interest,
            );
            return;
        }
        if let Err(e) = handler.ok().await {
            tracing::error!(error = ?e, "SUBSCRIBE_NAMESPACE failed");
            return;
        }
        let local_namespaces = self
            .forward_matching_publications(
                session_id,
                track_namespace_prefix,
                subscription,
                forwarder,
                downstream_publish,
            )
            .await;
        if subscription.interest.includes_namespaces() {
            self.notify_remote_publish_namespaces(
                session_id,
                track_namespace_prefix,
                &local_namespaces,
                forwarder,
                route_registry,
            )
            .await;
        }
    }

    #[tracing::instrument(
        level = "debug",
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
                        RequestErrorCode::NamespacePrefixOverlap,
                        "namespace already subscribed".to_string(),
                    )
                    .await
                {
                    Ok(_) => tracing::debug!("sent SUBSCRIBE_NAMESPACE_ERROR"),
                    Err(_) => tracing::error!("failed to send `SUBSCRIBE_NAMESPACE_ERROR`"),
                }
                false
            }
            Err(err) => {
                tracing::warn!(error = ?err, track_namespace_prefix = %track_namespace_prefix, "failed to register namespace subscription route");
                false
            }
        }
    }

    #[tracing::instrument(
        level = "debug",
        name = "relay.sequence.subscribe_namespace.forward_matching_publications",
        skip_all,
        fields(session_id = %session_id, track_namespace_prefix = %track_namespace_prefix)
    )]
    /// Returns the namespaces it sent PUBLISH_NAMESPACE for.
    async fn forward_matching_publications(
        &self,
        session_id: SessionId,
        track_namespace_prefix: &str,
        subscription: NamespaceSubscription,
        forwarder: &ControlMessageForwarder,
        downstream_publish: &DownstreamPublish,
    ) -> HashSet<String> {
        let publications = downstream_publish
            .table
            .matching_publications(track_namespace_prefix);
        let mut namespaces = HashSet::new();
        for publication in publications {
            match publication {
                MatchingPublication::Track(track_key)
                    if downstream_publish.table.offers_track(
                        session_id,
                        &subscription,
                        &track_key,
                    ) =>
                {
                    let _publish_task = DownstreamPublishTask::run(
                        downstream_publish.clone(),
                        session_id,
                        track_key,
                        subscription.forward,
                    );
                }
                MatchingPublication::Namespace(track_namespace)
                    if subscription.interest.includes_namespaces() =>
                {
                    forwarder
                        .publish_namespace(session_id, track_namespace.clone())
                        .await;
                    namespaces.insert(track_namespace);
                }
                MatchingPublication::Track(_) | MatchingPublication::Namespace(_) => {}
            }
        }
        namespaces
    }

    #[tracing::instrument(
        level = "debug",
        name = "relay.sequence.subscribe_namespace.notify_remote_publish_namespaces",
        skip_all,
        fields(session_id = %session_id, track_namespace_prefix = %track_namespace_prefix)
    )]
    /// The route registry lists this relay's own namespaces too, which
    /// `already_sent` holds.
    async fn notify_remote_publish_namespaces(
        &self,
        session_id: SessionId,
        track_namespace_prefix: &str,
        already_sent: &HashSet<String>,
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
                    error = ?err,
                    session_id = session_id,
                    track_namespace_prefix = %track_namespace_prefix,
                    "failed to find remote namespace routes"
                );
                return;
            }
        };

        for route in routes {
            if !already_sent.contains(&route.track_namespace) {
                forwarder
                    .publish_namespace(session_id, route.track_namespace)
                    .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::modules::test_support::directory_fixtures::unused_inter_relay_client_config;
    use std::{sync::Arc, time::Duration};

    use moqt::namespace::NamespaceInterest;

    use super::*;
    use crate::modules::{
        cascading::{
            inter_relay_connection_manager::InterRelayConnectionManager,
            route_registry::NoopRelayRouteRegistry,
        },
        control_plane::sequences::{CascadingRelayContext, publish_namespace::PublishNamespace},
        directory::{InMemoryLocalPubSubDirectory, track::ProducerOrigin},
        domain::session_peer::SessionPeer,
        test_support::{
            directory_fixtures::{
                DownstreamPublishContext, NAMESPACE_SUBSCRIBER_SESSION, PUBLISHER_SESSION,
                downstream_publish_context, local_publisher_resolver, namespaces_and_tracks,
                table_with_upstream, track_key,
            },
            mock_session::{
                MockPublishHandler, MockPublishNamespaceHandler, MockSubscribeNamespaceHandler,
                PUBLISH_REQUEST_ID, SentMessages, detached_session_repository,
            },
            stub_route_registry::StubRouteRegistry,
        },
    };

    /// Publishes `ns/alice` from inside SUBSCRIBE_NAMESPACE_OK: after the
    /// subscription is registered and before the existing namespaces are read.
    struct AnnouncingWhileAnswered {
        ctx: DownstreamPublishContext,
    }

    #[async_trait::async_trait]
    impl SubscribeNamespaceHandler for AnnouncingWhileAnswered {
        fn trace_context(&self) -> Option<&moqt::TraceContext> {
            None
        }
        fn track_namespace_prefix(&self) -> &str {
            "ns"
        }

        fn track_namespace_prefix_tuple(&self) -> &[String] {
            &[]
        }

        fn interest(&self) -> NamespaceInterest {
            NamespaceInterest::NamespacesAndTracks
        }

        fn forward(&self) -> bool {
            true
        }

        async fn ok(&self) -> Result<(), moqt::TransportSendError> {
            let inter_relay_connection_manager = InterRelayConnectionManager::new(
                Arc::new(tokio::sync::Mutex::new(detached_session_repository())),
                unused_inter_relay_client_config(),
            );
            PublishNamespace
                .handle(
                    PUBLISHER_SESSION,
                    &self.ctx.table,
                    &self.ctx.forwarder,
                    &self.ctx.cache_store,
                    &Arc::new(local_publisher_resolver()),
                    CascadingRelayContext {
                        route_registry: &NoopRelayRouteRegistry,
                        inter_relay_connection_manager: &inter_relay_connection_manager,
                    },
                    &MockPublishNamespaceHandler::new("ns/alice"),
                )
                .await;
            Ok(())
        }

        async fn error(
            &self,
            _code: RequestErrorCode,
            _reason_phrase: String,
        ) -> Result<(), moqt::TransportSendError> {
            Ok(())
        }
    }

    async fn context_with_published_track(sent: SentMessages) -> DownstreamPublishContext {
        let table = Arc::new(table_with_upstream(ProducerOrigin::Publish).0);
        table.register_publish(
            PUBLISHER_SESSION,
            SessionPeer::Client,
            Arc::new(MockPublishHandler::new("ns", "track", 0)),
        );
        downstream_publish_context(table, move || {
            sent.lock().unwrap().push("PUBLISH");
            Ok(true)
        })
        .await
    }

    async fn subscribe_namespace_until_publish_accepted(
        ctx: &mut DownstreamPublishContext,
        sent: SentMessages,
    ) {
        SubscribeNameSpace
            .handle(
                NAMESPACE_SUBSCRIBER_SESSION,
                &ctx.table,
                &ctx.forwarder,
                &ctx.downstream_publish(),
                &NoopRelayRouteRegistry,
                &MockSubscribeNamespaceHandler {
                    sent,
                    ..MockSubscribeNamespaceHandler::new("ns")
                },
            )
            .await;
        ctx.wait_registered(PUBLISH_REQUEST_ID).await;
    }

    #[tokio::test]
    async fn subscribe_namespace_ok_is_sent_before_existing_publishes() {
        // Arrange
        let sent = SentMessages::default();
        let mut ctx = context_with_published_track(sent.clone()).await;

        // Act
        subscribe_namespace_until_publish_accepted(&mut ctx, sent.clone()).await;

        // Assert
        assert_eq!(
            *sent.lock().unwrap(),
            vec!["SUBSCRIBE_NAMESPACE_OK", "PUBLISH"]
        );
    }

    #[tokio::test]
    async fn existing_publish_accepted_by_a_new_namespace_subscriber_is_a_downstream_subscription()
    {
        // Arrange
        let sent = SentMessages::default();
        let mut ctx = context_with_published_track(sent.clone()).await;

        // Act
        subscribe_namespace_until_publish_accepted(&mut ctx, sent).await;

        // Assert
        let registered = ctx
            .table
            .get_downstream_subscription(NAMESPACE_SUBSCRIBER_SESSION, PUBLISH_REQUEST_ID)
            .expect("PUBLISH_OK should register a downstream subscription");
        assert_eq!(registered.track_key, track_key());
    }

    #[tokio::test]
    async fn a_subscribe_namespace_overlapping_the_session_s_own_is_rejected() {
        // Arrange
        let sent = SentMessages::default();
        let mut ctx = context_with_published_track(sent.clone()).await;
        subscribe_namespace_until_publish_accepted(&mut ctx, sent.clone()).await;

        // Act
        SubscribeNameSpace
            .handle(
                NAMESPACE_SUBSCRIBER_SESSION,
                &ctx.table,
                &ctx.forwarder,
                &ctx.downstream_publish(),
                &NoopRelayRouteRegistry,
                &MockSubscribeNamespaceHandler {
                    sent: sent.clone(),
                    ..MockSubscribeNamespaceHandler::new("ns/alice")
                },
            )
            .await;

        // Assert
        assert_eq!(
            *sent.lock().unwrap(),
            vec![
                "SUBSCRIBE_NAMESPACE_OK",
                "PUBLISH",
                "SUBSCRIBE_NAMESPACE_ERROR"
            ]
        );
    }

    #[tokio::test]
    async fn a_namespace_published_here_and_in_the_route_registry_is_announced_once() {
        // Arrange: the registry lists this relay's own `ns/alice` next to `ns/bob` on another relay
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        table.register_publish_namespace(
            PUBLISHER_SESSION,
            "ns/alice".to_string(),
            SessionPeer::Client,
        );
        let ctx = downstream_publish_context(table, || Ok(true)).await;
        let sent = SentMessages::default();

        // Act
        SubscribeNameSpace
            .handle(
                NAMESPACE_SUBSCRIBER_SESSION,
                &ctx.table,
                &ctx.forwarder,
                &ctx.downstream_publish(),
                &StubRouteRegistry::with_namespace_routes(&["ns/alice", "ns/bob"]),
                &MockSubscribeNamespaceHandler {
                    sent,
                    ..MockSubscribeNamespaceHandler::new("ns")
                },
            )
            .await;

        // Assert
        assert_eq!(
            ctx.subscriber.publish_namespaces(),
            vec!["ns/alice", "ns/bob"]
        );
    }

    #[tokio::test]
    async fn a_namespace_published_while_subscribe_namespace_is_answered_is_announced_once() {
        // Arrange
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        let handler = AnnouncingWhileAnswered {
            ctx: downstream_publish_context(table, || Ok(true)).await,
        };
        let ctx = &handler.ctx;

        // Act
        SubscribeNameSpace
            .handle(
                NAMESPACE_SUBSCRIBER_SESSION,
                &ctx.table,
                &ctx.forwarder,
                &ctx.downstream_publish(),
                &NoopRelayRouteRegistry,
                &handler,
            )
            .await;

        // Assert
        assert_eq!(ctx.subscriber.publish_namespaces(), vec!["ns/alice"]);
    }

    /// How long a PUBLISH that should not come is waited for. The tests that
    /// wait run on paused time, which advances only once every task is
    /// idle, so a PUBLISH on its way is sent before the wait ends.
    const NO_PUBLISH_TIMEOUT: Duration = Duration::from_millis(200);

    async fn subscribe_namespace(
        ctx: &DownstreamPublishContext,
        handler: &MockSubscribeNamespaceHandler,
    ) {
        SubscribeNameSpace
            .handle(
                NAMESPACE_SUBSCRIBER_SESSION,
                &ctx.table,
                &ctx.forwarder,
                &ctx.downstream_publish(),
                &NoopRelayRouteRegistry,
                handler,
            )
            .await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_namespaces_subscription_is_sent_no_publish() {
        // Arrange
        let ctx = context_with_published_track(SentMessages::default()).await;
        let handler = MockSubscribeNamespaceHandler {
            interest: NamespaceInterest::Namespaces,
            ..MockSubscribeNamespaceHandler::new("ns")
        };

        // Act
        subscribe_namespace(&ctx, &handler).await;
        tokio::time::sleep(NO_PUBLISH_TIMEOUT).await;

        // Assert: draft-18 §6.1 sends PUBLISH for SUBSCRIBE_TRACKS only
        assert!(ctx.subscriber.publishes().is_empty());
    }

    #[tokio::test]
    async fn a_tracks_subscription_is_sent_the_publish_with_its_forward_state() {
        // Arrange
        let ctx = context_with_published_track(SentMessages::default()).await;
        let handler = MockSubscribeNamespaceHandler {
            interest: NamespaceInterest::Tracks,
            forward: false,
            ..MockSubscribeNamespaceHandler::new("ns")
        };

        // Act
        subscribe_namespace(&ctx, &handler).await;
        ctx.wait_registered(PUBLISH_REQUEST_ID).await;

        // Assert: draft-18 §10.19
        let publishes = ctx.subscriber.publishes();
        assert_eq!(publishes.len(), 1);
        assert!(!publishes[0].forward);
    }

    #[tokio::test(start_paused = true)]
    async fn a_tracks_subscription_is_not_sent_a_track_its_session_publishes() {
        // Arrange
        let ctx = context_with_published_track(SentMessages::default()).await;
        ctx.table.register_publish(
            NAMESPACE_SUBSCRIBER_SESSION,
            SessionPeer::Client,
            Arc::new(MockPublishHandler::new("ns", "track", 1)),
        );
        let handler = MockSubscribeNamespaceHandler {
            interest: NamespaceInterest::Tracks,
            ..MockSubscribeNamespaceHandler::new("ns")
        };

        // Act
        subscribe_namespace(&ctx, &handler).await;
        tokio::time::sleep(NO_PUBLISH_TIMEOUT).await;

        // Assert: draft-18 §6.1 leaves out the tracks published by the subscriber
        assert!(ctx.subscriber.publishes().is_empty());
    }

    #[tokio::test]
    async fn a_draft14_namespace_subscription_is_echoed_a_track_its_session_publishes() {
        // Arrange
        let ctx = context_with_published_track(SentMessages::default()).await;
        ctx.table.register_publish(
            NAMESPACE_SUBSCRIBER_SESSION,
            SessionPeer::Client,
            Arc::new(MockPublishHandler::new("ns", "track", 1)),
        );
        let handler = MockSubscribeNamespaceHandler::new("ns");

        // Act
        subscribe_namespace(&ctx, &handler).await;
        ctx.wait_registered(PUBLISH_REQUEST_ID).await;

        // Assert: draft-14 §6.1 echoes back the tracks published to the relay
        assert_eq!(ctx.subscriber.publishes().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_tracks_subscription_is_sent_only_tracks_that_arrived_by_publish() {
        // Arrange: the track is known from an upstream SUBSCRIBE to a PUBLISH_NAMESPACE publisher
        let table = Arc::new(table_with_upstream(ProducerOrigin::Subscribe).0);
        table.register_publish_namespace(PUBLISHER_SESSION, "ns".to_string(), SessionPeer::Client);
        let ctx = downstream_publish_context(table, || Ok(true)).await;
        let handler = MockSubscribeNamespaceHandler {
            interest: NamespaceInterest::Tracks,
            ..MockSubscribeNamespaceHandler::new("ns")
        };

        // Act
        subscribe_namespace(&ctx, &handler).await;
        tokio::time::sleep(NO_PUBLISH_TIMEOUT).await;

        // Assert: a relay sends PUBLISH for the tracks published to it
        assert!(ctx.subscriber.publishes().is_empty());
    }

    async fn context_holding_namespace_subscription(prefix: &str) -> DownstreamPublishContext {
        let ctx = context_with_published_track(SentMessages::default()).await;
        ctx.table.register_subscribe_namespace(
            NAMESPACE_SUBSCRIBER_SESSION,
            prefix.to_string(),
            namespaces_and_tracks(SessionPeer::Client),
        );
        ctx
    }

    #[tokio::test(start_paused = true)]
    async fn a_wider_namespace_subscription_after_a_narrower_one_is_refused_with_prefix_overlap() {
        // Arrange
        let ctx = context_holding_namespace_subscription("ns/track").await;
        let handler = MockSubscribeNamespaceHandler::new("ns");

        // Act
        subscribe_namespace(&ctx, &handler).await;
        tokio::time::sleep(NO_PUBLISH_TIMEOUT).await;

        // Assert: draft-14 §9.28 / draft-18 §10.18
        assert_eq!(*handler.sent.lock().unwrap(), ["SUBSCRIBE_NAMESPACE_ERROR"]);
        assert_eq!(
            *handler.error_codes.lock().unwrap(),
            [RequestErrorCode::NamespacePrefixOverlap]
        );
        assert!(ctx.table.subscriber_namespaces.get("ns").is_none());
        assert!(ctx.subscriber.publishes().is_empty());
    }

    #[tokio::test]
    async fn a_narrower_namespace_subscription_after_a_wider_one_is_refused_with_prefix_overlap() {
        // Arrange
        let ctx = context_holding_namespace_subscription("ns").await;
        let handler = MockSubscribeNamespaceHandler::new("ns/track");

        // Act
        subscribe_namespace(&ctx, &handler).await;

        // Assert: draft-14 §9.28 / draft-18 §10.18
        assert_eq!(*handler.sent.lock().unwrap(), ["SUBSCRIBE_NAMESPACE_ERROR"]);
        assert_eq!(
            *handler.error_codes.lock().unwrap(),
            [RequestErrorCode::NamespacePrefixOverlap]
        );
        assert!(ctx.table.subscriber_namespaces.get("ns/track").is_none());
    }
}
