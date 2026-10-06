use crate::modules::{
    cascading::route_registry::{RegisterRouteError, RelayRouteRegistry},
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        sequences::downstream_publish::DownstreamPublish,
    },
    domain::{
        error_code::SubscribeNamespaceErrorCode,
        pub_sub_directory::{InMemoryLocalPubSubDirectory, entry::MatchingPublication},
        session_id::SessionId,
    },
    session::handler::subscribe_namespace::SubscribeNamespaceHandler,
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
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        downstream_publish: &DownstreamPublish<'_>,
        route_registry: &dyn RelayRouteRegistry,
        handler: &dyn SubscribeNamespaceHandler,
    ) {
        let track_namespace_prefix = handler.track_namespace_prefix();
        tracing::info!(
            session_id = %session_id,
            track_namespace_prefix = %track_namespace_prefix,
            "SequenceHandler::SubscribeNamespace"
        );
        let peer = super::session_peer(session_id, forwarder).await;
        let is_first_client = table.register_subscribe_namespace(
            session_id,
            track_namespace_prefix.to_string(),
            peer,
        );
        if is_first_client
            && !self
                .register_route(route_registry, track_namespace_prefix, handler)
                .await
        {
            table.unregister_subscribe_namespace(session_id, track_namespace_prefix);
            return;
        }
        if let Err(e) = handler.ok().await {
            tracing::error!("Subscribe Namespace Error: {:?}", e);
            return;
        }
        self.forward_matching_publications(
            session_id,
            track_namespace_prefix,
            forwarder,
            downstream_publish,
        )
        .await;
        self.notify_remote_publish_namespaces(
            session_id,
            track_namespace_prefix,
            forwarder,
            route_registry,
        )
        .await;
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
        name = "relay.sequence.subscribe_namespace.forward_matching_publications",
        skip_all,
        fields(session_id = %session_id, track_namespace_prefix = %track_namespace_prefix)
    )]
    async fn forward_matching_publications(
        &self,
        session_id: SessionId,
        track_namespace_prefix: &str,
        forwarder: &ControlMessageForwarder,
        downstream_publish: &DownstreamPublish<'_>,
    ) {
        let publications = downstream_publish
            .table
            .matching_publications(track_namespace_prefix);
        for publication in publications {
            match publication {
                MatchingPublication::Track(track_key) => {
                    match downstream_publish.send(session_id, &track_key).await {
                        Ok(()) => tracing::info!(track_key = %track_key, "forwarded PUBLISH"),
                        Err(error) => {
                            tracing::warn!(?error, track_key = %track_key, "failed to forward PUBLISH")
                        }
                    }
                }
                MatchingPublication::Namespace(track_namespace) => {
                    if forwarder
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::modules::{
        cascading::route_registry::NoopRelayRouteRegistry,
        domain::{pub_sub_directory::entry::UpstreamSubscriptionOrigin, session_peer::SessionPeer},
        test_support::{
            directory_fixtures::{
                DownstreamPublishContext, NAMESPACE_SUBSCRIBER_SESSION, PUBLISHER_SESSION,
                downstream_publish_context, table_with_upstream, track_key,
            },
            mock_session::{MockPublishHandler, PUBLISH_REQUEST_ID},
        },
    };

    type SentMessages = Arc<Mutex<Vec<&'static str>>>;

    struct MockSubscribeNamespaceHandler {
        sent: SentMessages,
    }

    #[async_trait::async_trait]
    impl SubscribeNamespaceHandler for MockSubscribeNamespaceHandler {
        fn track_namespace_prefix(&self) -> &str {
            "ns"
        }

        fn track_namespace_prefix_tuple(&self) -> &[String] {
            &[]
        }

        async fn ok(&self) -> Result<(), moqt::TransportSendError> {
            self.sent.lock().unwrap().push("SUBSCRIBE_NAMESPACE_OK");
            Ok(())
        }

        async fn error(
            &self,
            _code: u64,
            _reason_phrase: String,
        ) -> Result<(), moqt::TransportSendError> {
            self.sent.lock().unwrap().push("SUBSCRIBE_NAMESPACE_ERROR");
            Ok(())
        }
    }

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

    async fn subscribe_namespace(ctx: &DownstreamPublishContext, sent: SentMessages) {
        SubscribeNameSpace
            .handle(
                NAMESPACE_SUBSCRIBER_SESSION,
                &Span::none(),
                &ctx.table,
                &ctx.forwarder,
                &ctx.downstream_publish(),
                &NoopRelayRouteRegistry,
                &MockSubscribeNamespaceHandler { sent },
            )
            .await;
    }

    #[tokio::test]
    async fn subscribe_namespace_ok_is_sent_before_existing_publishes() {
        // Arrange
        let sent = SentMessages::default();
        let ctx = context_with_published_track(sent.clone()).await;

        // Act
        subscribe_namespace(&ctx, sent.clone()).await;

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
        let ctx = context_with_published_track(sent.clone()).await;

        // Act
        subscribe_namespace(&ctx, sent).await;

        // Assert
        let registered = ctx
            .table
            .get_downstream_subscription(NAMESPACE_SUBSCRIBER_SESSION, PUBLISH_REQUEST_ID)
            .expect("PUBLISH_OK should register a downstream subscription");
        assert_eq!(registered.track_key, track_key());
    }
}
