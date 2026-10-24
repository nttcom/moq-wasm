use crate::modules::{
    cascading::route_registry::{RegisterRouteError, RelayRouteRegistry},
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        sequences::{
            CascadingRelayContext,
            downstream_publish::{
                DownstreamPublish, downstream_publish_task::DownstreamPublishTask,
            },
            unsubscribe_namespace::UnsubscribeNamespace,
        },
    },
    domain::{
        pub_sub_directory::{
            InMemoryLocalPubSubDirectory,
            entry::{MatchingPublication, NamespaceSubscription},
        },
        session_id::SessionId,
        session_peer::SessionPeer,
    },
    session::handler::{
        subscribe_namespace::SubscribeNamespaceHandler,
        subscribe_namespace_update::SubscribeNamespaceUpdateHandler,
    },
};
use moqt::RequestErrorCode;
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
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
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
            "SequenceHandler::SubscribeNamespace"
        );
        if table.has_overlapping_namespace_subscription(
            session_id,
            track_namespace_prefix,
            subscription.interest,
            None,
        ) {
            tracing::info!(
                track_namespace_prefix = %track_namespace_prefix,
                "namespace subscription overlaps another of the session"
            );
            if let Err(error) = handler
                .error(
                    RequestErrorCode::NamespacePrefixOverlap,
                    "overlaps another namespace subscription of the session".to_string(),
                )
                .await
            {
                tracing::error!(
                    ?error,
                    "failed to refuse an overlapping namespace subscription"
                );
            }
            return;
        }
        let is_first_client = table.register_subscribe_namespace(
            session_id,
            track_namespace_prefix.to_string(),
            subscription,
        );
        if is_first_client
            && let Err(error) = self
                .register_route(route_registry, track_namespace_prefix)
                .await
        {
            table.unregister_subscribe_namespace(
                session_id,
                track_namespace_prefix,
                subscription.interest,
            );
            if let RegisterRouteError::Conflict = error {
                match handler
                    .error(
                        RequestErrorCode::NamespacePrefixOverlap,
                        "namespace already subscribed".to_string(),
                    )
                    .await
                {
                    Ok(_) => tracing::info!("sent `SUBSCRIBE_NAMESPACE_ERROR` ok"),
                    Err(_) => tracing::error!("failed to send `SUBSCRIBE_NAMESPACE_ERROR`"),
                }
            }
            return;
        }
        if let Err(e) = handler.ok().await {
            tracing::error!("Subscribe Namespace Error: {:?}", e);
            return;
        }
        self.forward_matching_publications(
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
                forwarder,
                route_registry,
            )
            .await;
        }
    }

    /// draft-18 §10.9.2: moves the session's subscription of the handler's
    /// interest to the new prefix, or refuses it with PREFIX_OVERLAP
    /// (§10.2.14). Tracks the session already receives are not PUBLISHed
    /// again, and moqt announces each namespace once per subscription. A
    /// refused update ends the subscription, which moqt reports as
    /// UNSUBSCRIBE_NAMESPACE.
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe_namespace_update",
        skip_all,
        parent = session_span,
        fields(session_id = %session_id)
    )]
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle_update(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        downstream_publish: &DownstreamPublish,
        cascading_relay_context: CascadingRelayContext<'_>,
        handler: &dyn SubscribeNamespaceUpdateHandler,
    ) {
        let track_namespace_prefix = handler.track_namespace_prefix();
        let new_track_namespace_prefix = handler.new_track_namespace_prefix();
        let interest = handler.interest();
        tracing::info!(
            track_namespace_prefix = %track_namespace_prefix,
            new_track_namespace_prefix = %new_track_namespace_prefix,
            ?interest,
            "SequenceHandler::SubscribeNamespaceUpdate"
        );
        let Some(subscription) =
            table.namespace_subscription(session_id, track_namespace_prefix, interest)
        else {
            tracing::warn!("update of a namespace subscription the session does not hold");
            handler
                .error(
                    RequestErrorCode::InternalError,
                    "no such namespace subscription".to_string(),
                )
                .await;
            return;
        };
        // Nothing moves; registering and then releasing the same prefix
        // would drop the subscription.
        if new_track_namespace_prefix == track_namespace_prefix {
            handler.ok().await;
            return;
        }
        if table.has_overlapping_namespace_subscription(
            session_id,
            new_track_namespace_prefix,
            interest,
            Some(track_namespace_prefix),
        ) {
            tracing::info!("updated prefix overlaps another namespace subscription of the session");
            handler
                .error(
                    RequestErrorCode::NamespacePrefixOverlap,
                    "overlaps another namespace subscription of the session".to_string(),
                )
                .await;
            return;
        }
        let is_first_client = table.register_subscribe_namespace(
            session_id,
            new_track_namespace_prefix.to_string(),
            subscription,
        );
        if is_first_client
            && let Err(error) = self
                .register_route(
                    cascading_relay_context.route_registry,
                    new_track_namespace_prefix,
                )
                .await
        {
            table.unregister_subscribe_namespace(session_id, new_track_namespace_prefix, interest);
            let (code, reason) = match error {
                RegisterRouteError::Conflict => (
                    RequestErrorCode::NamespacePrefixOverlap,
                    "namespace already subscribed",
                ),
                _ => (
                    RequestErrorCode::InternalError,
                    "namespace route not registered",
                ),
            };
            handler.error(code, reason.to_string()).await;
            return;
        }
        handler.ok().await;
        let no_clients_remain =
            table.unregister_subscribe_namespace(session_id, track_namespace_prefix, interest);
        if no_clients_remain && subscription.peer == SessionPeer::Client {
            UnsubscribeNamespace::cleanup_empty_namespace_subscription(
                track_namespace_prefix,
                table,
                forwarder,
                cascading_relay_context.route_registry,
                cascading_relay_context.inter_relay_connection_manager,
            )
            .await;
        }
        self.forward_matching_publications(
            session_id,
            new_track_namespace_prefix,
            subscription,
            forwarder,
            downstream_publish,
        )
        .await;
        if interest.includes_namespaces() {
            self.notify_remote_publish_namespaces(
                session_id,
                new_track_namespace_prefix,
                forwarder,
                cascading_relay_context.route_registry,
            )
            .await;
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
    ) -> Result<(), RegisterRouteError> {
        let registered = route_registry
            .register_namespace_subscriber(track_namespace_prefix)
            .await;
        match &registered {
            Ok(()) => {}
            Err(RegisterRouteError::Conflict) => {
                tracing::warn!(track_namespace_prefix = %track_namespace_prefix, "namespace already has an active subscriber");
            }
            Err(err) => {
                tracing::warn!(?err, track_namespace_prefix = %track_namespace_prefix, "failed to register namespace subscription route");
            }
        }
        registered
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe_namespace.forward_matching_publications",
        skip_all,
        fields(session_id = %session_id, track_namespace_prefix = %track_namespace_prefix)
    )]
    async fn forward_matching_publications(
        &self,
        session_id: SessionId,
        track_namespace_prefix: &str,
        subscription: NamespaceSubscription,
        forwarder: &ControlMessageForwarder,
        downstream_publish: &DownstreamPublish,
    ) {
        let publications = downstream_publish
            .table
            .matching_publications(track_namespace_prefix);
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
                }
                MatchingPublication::Track(_) | MatchingPublication::Namespace(_) => {}
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
            forwarder
                .publish_namespace(session_id, route.track_namespace.clone())
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use moqt::namespace::NamespaceInterest;

    use super::*;
    use crate::modules::{
        cascading::{
            inter_relay_connection_manager::InterRelayConnectionManager,
            route_registry::NoopRelayRouteRegistry,
        },
        domain::{pub_sub_directory::entry::UpstreamSubscriptionOrigin, session_peer::SessionPeer},
        session::session_repository::SessionRepository,
        test_support::{
            directory_fixtures::{
                DownstreamPublishContext, NAMESPACE_SUBSCRIBER_SESSION, PUBLISHER_SESSION,
                client_subscription, downstream_publish_context, namespaces_and_tracks,
                table_with_upstream, track_key,
            },
            mock_session::{
                MockPublishHandler, MockSubscribeNamespaceHandler,
                MockSubscribeNamespaceUpdateHandler, PUBLISH_REQUEST_ID, SentMessages,
            },
        },
    };

    /// How long a PUBLISH that should not come is waited for.
    const NO_PUBLISH_TIMEOUT: Duration = Duration::from_millis(200);

    async fn context_with_published_track(sent: SentMessages) -> DownstreamPublishContext {
        let table = Arc::new(table_with_upstream(UpstreamSubscriptionOrigin::Publish).0);
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
                &Span::none(),
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
        tokio::time::timeout(Duration::from_secs(3), ctx.egress_receiver.recv())
            .await
            .expect("the accepted PUBLISH should start an egress reader");
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
        let (registered, _) = ctx
            .table
            .get_downstream_subscription(NAMESPACE_SUBSCRIBER_SESSION, PUBLISH_REQUEST_ID)
            .expect("PUBLISH_OK should register a downstream subscription");
        assert_eq!(registered.track_key, track_key());
    }

    async fn subscribe_namespace(
        ctx: &DownstreamPublishContext,
        handler: MockSubscribeNamespaceHandler,
    ) {
        SubscribeNameSpace
            .handle(
                NAMESPACE_SUBSCRIBER_SESSION,
                &Span::none(),
                &ctx.table,
                &ctx.forwarder,
                &ctx.downstream_publish(),
                &NoopRelayRouteRegistry,
                &handler,
            )
            .await;
    }

    #[tokio::test]
    async fn a_namespaces_subscription_is_sent_no_publish() {
        // Arrange
        let mut ctx = context_with_published_track(SentMessages::default()).await;
        let handler = MockSubscribeNamespaceHandler {
            interest: NamespaceInterest::Namespaces,
            ..MockSubscribeNamespaceHandler::new("ns")
        };

        // Act
        subscribe_namespace(&ctx, handler).await;
        let egress = tokio::time::timeout(NO_PUBLISH_TIMEOUT, ctx.egress_receiver.recv()).await;

        // Assert: draft-18 §6.1 sends PUBLISH for SUBSCRIBE_TRACKS only
        assert!(egress.is_err());
        assert!(ctx.subscriber.publishes().is_empty());
    }

    #[tokio::test]
    async fn a_tracks_subscription_is_sent_the_publish_with_its_forward_state() {
        // Arrange
        let mut ctx = context_with_published_track(SentMessages::default()).await;
        let handler = MockSubscribeNamespaceHandler {
            interest: NamespaceInterest::Tracks,
            forward: false,
            ..MockSubscribeNamespaceHandler::new("ns")
        };

        // Act
        subscribe_namespace(&ctx, handler).await;
        tokio::time::timeout(Duration::from_secs(3), ctx.egress_receiver.recv())
            .await
            .expect("the accepted PUBLISH should start an egress reader");

        // Assert: draft-18 §10.19
        let publishes = ctx.subscriber.publishes();
        assert_eq!(publishes.len(), 1);
        assert!(!publishes[0].forward);
    }

    #[tokio::test]
    async fn a_tracks_subscription_is_not_sent_a_track_its_session_publishes() {
        // Arrange
        let mut ctx = context_with_published_track(SentMessages::default()).await;
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
        subscribe_namespace(&ctx, handler).await;
        let egress = tokio::time::timeout(NO_PUBLISH_TIMEOUT, ctx.egress_receiver.recv()).await;

        // Assert: draft-18 §6.1 leaves out the tracks published by the subscriber
        assert!(egress.is_err());
        assert!(ctx.subscriber.publishes().is_empty());
    }

    #[tokio::test]
    async fn an_overlapping_namespace_subscription_is_refused_with_prefix_overlap() {
        // Arrange
        let mut ctx = context_with_published_track(SentMessages::default()).await;
        ctx.table.register_subscribe_namespace(
            NAMESPACE_SUBSCRIBER_SESSION,
            "ns/track".to_string(),
            namespaces_and_tracks(SessionPeer::Client),
        );
        let handler = MockSubscribeNamespaceHandler::new("ns");

        // Act
        subscribe_namespace(&ctx, handler.clone()).await;
        let egress = tokio::time::timeout(NO_PUBLISH_TIMEOUT, ctx.egress_receiver.recv()).await;

        // Assert: draft-14 §9.28 / draft-18 §10.18
        assert_eq!(*handler.sent.lock().unwrap(), ["SUBSCRIBE_NAMESPACE_ERROR"]);
        assert_eq!(
            *handler.error_codes.lock().unwrap(),
            [RequestErrorCode::NamespacePrefixOverlap]
        );
        assert!(ctx.table.subscriber_namespaces.get("ns").is_none());
        assert!(egress.is_err());
    }

    async fn update_namespace_subscription(
        ctx: &DownstreamPublishContext,
        handler: &MockSubscribeNamespaceUpdateHandler,
    ) {
        let (session_event_sender, _session_event_receiver) =
            tokio::sync::mpsc::unbounded_channel();
        let inter_relay_connection_manager = InterRelayConnectionManager::new(
            Arc::new(tokio::sync::Mutex::new(SessionRepository::new())),
            session_event_sender,
            "unused-relay-token".to_string(),
        );
        SubscribeNameSpace
            .handle_update(
                NAMESPACE_SUBSCRIBER_SESSION,
                &Span::none(),
                &ctx.table,
                &ctx.forwarder,
                &ctx.downstream_publish(),
                CascadingRelayContext {
                    route_registry: &NoopRelayRouteRegistry,
                    inter_relay_connection_manager: &inter_relay_connection_manager,
                },
                handler,
            )
            .await;
    }

    #[tokio::test]
    async fn a_namespace_prefix_update_overlapping_another_subscription_is_refused() {
        // Arrange
        let ctx = context_with_published_track(SentMessages::default()).await;
        for prefix in ["ns", "other"] {
            ctx.table.register_subscribe_namespace(
                NAMESPACE_SUBSCRIBER_SESSION,
                prefix.to_string(),
                client_subscription(NamespaceInterest::Namespaces),
            );
        }
        let handler = MockSubscribeNamespaceUpdateHandler::new(
            "other",
            "ns/room",
            NamespaceInterest::Namespaces,
        );

        // Act
        update_namespace_subscription(&ctx, &handler).await;

        // Assert: draft-18 §10.2.14 PREFIX_OVERLAP; the subscription stays where it was
        assert_eq!(
            handler.answers(),
            [Some(RequestErrorCode::NamespacePrefixOverlap)]
        );
        assert!(
            ctx.table
                .namespace_subscription(
                    NAMESPACE_SUBSCRIBER_SESSION,
                    "other",
                    NamespaceInterest::Namespaces
                )
                .is_some()
        );
        assert!(ctx.table.subscriber_namespaces.get("ns/room").is_none());
    }

    #[tokio::test]
    async fn a_namespace_prefix_update_moves_the_subscription_and_offers_what_now_matches() {
        // Arrange: the old prefix overlaps only the subscription being moved
        let mut ctx = context_with_published_track(SentMessages::default()).await;
        ctx.table.register_subscribe_namespace(
            NAMESPACE_SUBSCRIBER_SESSION,
            "ns/old".to_string(),
            client_subscription(NamespaceInterest::Tracks),
        );
        let handler =
            MockSubscribeNamespaceUpdateHandler::new("ns/old", "ns", NamespaceInterest::Tracks);

        // Act
        update_namespace_subscription(&ctx, &handler).await;
        tokio::time::timeout(Duration::from_secs(3), ctx.egress_receiver.recv())
            .await
            .expect("the accepted PUBLISH should start an egress reader");

        // Assert: draft-18 §10.9.2
        assert_eq!(handler.answers(), [None]);
        assert!(ctx.table.subscriber_namespaces.get("ns/old").is_none());
        assert_eq!(
            ctx.table.namespace_subscription(
                NAMESPACE_SUBSCRIBER_SESSION,
                "ns",
                NamespaceInterest::Tracks
            ),
            Some(client_subscription(NamespaceInterest::Tracks))
        );
        assert_eq!(ctx.subscriber.publishes().len(), 1);
    }

    #[tokio::test]
    async fn a_namespace_prefix_update_to_the_same_prefix_keeps_the_subscription() {
        // Arrange
        let ctx = context_with_published_track(SentMessages::default()).await;
        ctx.table.register_subscribe_namespace(
            NAMESPACE_SUBSCRIBER_SESSION,
            "ns".to_string(),
            client_subscription(NamespaceInterest::Namespaces),
        );
        let handler =
            MockSubscribeNamespaceUpdateHandler::new("ns", "ns", NamespaceInterest::Namespaces);

        // Act
        update_namespace_subscription(&ctx, &handler).await;

        // Assert
        assert_eq!(handler.answers(), [None]);
        assert_eq!(
            ctx.table.namespace_subscription(
                NAMESPACE_SUBSCRIBER_SESSION,
                "ns",
                NamespaceInterest::Namespaces
            ),
            Some(client_subscription(NamespaceInterest::Namespaces))
        );
    }

    #[tokio::test]
    async fn a_namespace_prefix_update_may_share_the_prefix_of_the_other_kind() {
        // Arrange
        let ctx = context_with_published_track(SentMessages::default()).await;
        ctx.table.register_subscribe_namespace(
            NAMESPACE_SUBSCRIBER_SESSION,
            "ns".to_string(),
            client_subscription(NamespaceInterest::Namespaces),
        );
        ctx.table.register_subscribe_namespace(
            NAMESPACE_SUBSCRIBER_SESSION,
            "other".to_string(),
            client_subscription(NamespaceInterest::Tracks),
        );
        let handler =
            MockSubscribeNamespaceUpdateHandler::new("other", "ns", NamespaceInterest::Tracks);

        // Act
        update_namespace_subscription(&ctx, &handler).await;

        // Assert: draft-18 §10.9.2 applies the overlap rule per type
        assert_eq!(handler.answers(), [None]);
    }
}
