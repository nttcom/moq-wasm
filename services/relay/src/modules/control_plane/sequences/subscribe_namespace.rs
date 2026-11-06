use crate::modules::{
    cascading::route_registry::{RegisterRouteError, RelayRouteRegistry},
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        sequences::downstream_publish::{
            DownstreamPublish, downstream_publish_task::DownstreamPublishTask,
        },
    },
    directory::{InMemoryLocalPubSubDirectory, track::MatchingPublication},
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
        tracing::info!(
            session_id = %session_id,
            track_namespace_prefix = %track_namespace_prefix,
            "SUBSCRIBE_NAMESPACE received"
        );
        if table.has_overlapping_subscribe_namespace(session_id, track_namespace_prefix) {
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
            tracing::error!(error = ?e, "SUBSCRIBE_NAMESPACE failed");
            return;
        }
        let local_namespaces = self
            .forward_matching_publications(
                session_id,
                track_namespace_prefix,
                forwarder,
                downstream_publish,
            )
            .await;
        self.notify_remote_publish_namespaces(
            session_id,
            track_namespace_prefix,
            &local_namespaces,
            forwarder,
            route_registry,
        )
        .await;
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
        forwarder: &ControlMessageForwarder,
        downstream_publish: &DownstreamPublish,
    ) -> HashSet<String> {
        let publications = downstream_publish
            .table
            .matching_publications(track_namespace_prefix);
        let mut namespaces = HashSet::new();
        for publication in publications {
            match publication {
                MatchingPublication::Track(track_key) => {
                    let _publish_task = DownstreamPublishTask::run(
                        downstream_publish.clone(),
                        session_id,
                        track_key,
                    );
                }
                MatchingPublication::Namespace(track_namespace) => {
                    forwarder
                        .publish_namespace(session_id, track_namespace.clone())
                        .await;
                    namespaces.insert(track_namespace);
                }
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
    use std::sync::{Arc, Mutex};

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
                downstream_publish_context, local_publisher_resolver, table_with_upstream,
                track_key,
            },
            mock_session::{
                MockPublishHandler, MockPublishNamespaceHandler, PUBLISH_REQUEST_ID,
                detached_session_repository,
            },
            stub_route_registry::StubRouteRegistry,
        },
    };

    type SentMessages = Arc<Mutex<Vec<&'static str>>>;

    struct MockSubscribeNamespaceHandler {
        prefix: &'static str,
        sent: SentMessages,
    }

    #[async_trait::async_trait]
    impl SubscribeNamespaceHandler for MockSubscribeNamespaceHandler {
        fn trace_context(&self) -> Option<&moqt::TraceContext> {
            None
        }
        fn track_namespace_prefix(&self) -> &str {
            self.prefix
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
            _code: RequestErrorCode,
            _reason_phrase: String,
        ) -> Result<(), moqt::TransportSendError> {
            self.sent.lock().unwrap().push("SUBSCRIBE_NAMESPACE_ERROR");
            Ok(())
        }
    }

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
                &MockSubscribeNamespaceHandler { prefix: "ns", sent },
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
                    prefix: "ns/alice",
                    sent: sent.clone(),
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
                &MockSubscribeNamespaceHandler { prefix: "ns", sent },
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
}
