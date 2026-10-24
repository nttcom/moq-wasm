use crate::modules::{
    cascading::route_registry::{RegisterRouteError, RelayRouteRegistry},
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        sequences::downstream_publish::{
            DownstreamPublish, downstream_publish_task::DownstreamPublishTask,
        },
    },
    domain::{
        pub_sub_directory::{
            InMemoryLocalPubSubDirectory,
            entry::{MatchingPublication, NamespaceSubscription},
        },
        session_id::SessionId,
    },
    session::handler::subscribe_namespace::SubscribeNamespaceHandler,
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
                        RequestErrorCode::NamespacePrefixOverlap,
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
        cascading::route_registry::NoopRelayRouteRegistry,
        domain::{pub_sub_directory::entry::UpstreamSubscriptionOrigin, session_peer::SessionPeer},
        test_support::{
            directory_fixtures::{
                DownstreamPublishContext, NAMESPACE_SUBSCRIBER_SESSION, PUBLISHER_SESSION,
                downstream_publish_context, namespaces_and_tracks, table_with_upstream, track_key,
            },
            mock_session::{
                MockPublishHandler, MockSubscribeNamespaceHandler, PUBLISH_REQUEST_ID, SentMessages,
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
}
