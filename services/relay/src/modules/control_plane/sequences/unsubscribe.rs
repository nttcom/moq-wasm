use crate::modules::{
    cascading::namespace_routes::NamespaceRoutes,
    control_plane::control_message_forwarder::ControlMessageForwarder,
    directory::InMemoryLocalPubSubDirectory,
    domain::{publish_done_reason::PublishDoneReason, session_id::SessionId},
};

pub(crate) struct Unsubscribe;

impl Unsubscribe {
    /// Removes the downstream subscription `subscribe_id` and releases the
    /// upstream subscriptions it leaves unneeded; runs for an UNSUBSCRIBE,
    /// for a subscription whose egress runner delivered its whole range, and
    /// for one the relay ends with PUBLISH_DONE `end` (see
    /// `InMemoryLocalPubSubDirectory::end_downstream_subscription`).
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        namespace_routes: &NamespaceRoutes,
        subscribe_id: u64,
        end: Option<PublishDoneReason>,
    ) {
        tracing::debug!(
            session_id = %session_id,
            request_id = %subscribe_id,
            "UNSUBSCRIBE received"
        );

        let Some(removed) = table.end_downstream_subscription(session_id, subscribe_id, end) else {
            tracing::warn!(
                session_id = %session_id,
                request_id = %subscribe_id,
                "active downstream subscription not found"
            );
            return;
        };

        tracing::info!(
            session_id = %session_id,
            request_id = %subscribe_id,
            track_namespace = %removed.track_key.track_namespace,
            track_name = %removed.track_key.track_name,
            released_upstream_subscriptions = removed.released_upstream_subscriptions.len(),
            "UNSUBSCRIBE handled"
        );

        super::release_upstream_subscriptions(
            table,
            forwarder,
            namespace_routes,
            &removed.track_key,
            &removed.released_upstream_subscriptions,
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use crate::modules::test_support::directory_fixtures::delivery_control;
    use crate::modules::test_support::directory_fixtures::downstream_subscription;
    use std::sync::Arc;

    use tokio::sync::oneshot;

    use super::*;
    use crate::modules::{
        directory::track::ProducerOrigin,
        domain::{publish_done_reason::PublishDoneReason, session_peer::SessionPeer},
        test_support::{
            directory_fixtures::{
                UPSTREAM_REQUEST_ID, UpstreamReleaseContext, register_downstream, track_key,
                upstream_release_context,
            },
            mock_session::runner_stopped,
            stub_route_registry::{PublisherLookup, StubRouteRegistry},
        },
    };

    struct TestContext {
        upstream: UpstreamReleaseContext,
        route_registry: Arc<StubRouteRegistry>,
        namespace_routes: NamespaceRoutes,
        runner_stop_receivers: Vec<oneshot::Receiver<PublishDoneReason>>,
    }

    async fn setup(
        origin: ProducerOrigin,
        downstream_subscriptions: &[(SessionId, u64)],
    ) -> TestContext {
        let upstream = upstream_release_context(origin).await;
        let runner_stop_receivers = downstream_subscriptions
            .iter()
            .map(|(session_id, subscribe_id)| {
                register_downstream(
                    &upstream.table,
                    *session_id,
                    *subscribe_id,
                    SessionPeer::Client,
                    downstream_subscription(track_key(), None),
                    delivery_control(true),
                )
                .unwrap()
                .stop_receiver
            })
            .collect();
        let route_registry = Arc::new(StubRouteRegistry::new(PublisherLookup::MustNotBeCalled));
        TestContext {
            upstream,
            namespace_routes: NamespaceRoutes::new(route_registry.clone()),
            route_registry,
            runner_stop_receivers,
        }
    }

    async fn run_unsubscribe(ctx: &TestContext, session_id: SessionId, subscribe_id: u64) {
        end_subscription(ctx, session_id, subscribe_id, None).await;
    }

    async fn end_subscription(
        ctx: &TestContext,
        session_id: SessionId,
        subscribe_id: u64,
        end: Option<PublishDoneReason>,
    ) {
        Unsubscribe
            .handle(
                session_id,
                &ctx.upstream.table,
                &ctx.upstream.forwarder,
                &ctx.namespace_routes,
                subscribe_id,
                end,
            )
            .await;
    }

    #[tokio::test]
    async fn ending_a_subscription_hands_its_runner_the_publish_done_and_releases_upstream() {
        // Arrange
        let mut ctx = setup(ProducerOrigin::Subscribe, &[(100, 10)]).await;

        // Act
        end_subscription(&ctx, 100, 10, Some(PublishDoneReason::update_failed())).await;

        // Assert: the runner answers it with PUBLISH_DONE UPDATE_FAILED
        assert_eq!(
            ctx.runner_stop_receivers[0].try_recv(),
            Ok(PublishDoneReason::update_failed())
        );
        assert_eq!(
            ctx.upstream.recorded.unsubscribed_request_ids(),
            vec![UPSTREAM_REQUEST_ID]
        );
    }

    #[tokio::test]
    async fn last_subscriber_forwards_upstream_unsubscribe_and_stops_ingress() {
        // Arrange
        let mut ctx = setup(ProducerOrigin::Subscribe, &[(100, 10)]).await;

        // Act
        run_unsubscribe(&ctx, 100, 10).await;

        // Assert
        assert!(runner_stopped(&mut ctx.runner_stop_receivers[0]));
        assert_eq!(
            ctx.upstream.recorded.unsubscribed_request_ids(),
            vec![UPSTREAM_REQUEST_ID]
        );
        ctx.upstream
            .recorded
            .wait_ingest_stopped(UPSTREAM_REQUEST_ID)
            .await;
    }

    #[tokio::test]
    async fn remaining_subscribers_keep_upstream_subscription() {
        // Arrange
        let mut ctx = setup(ProducerOrigin::Subscribe, &[(100, 10), (101, 11)]).await;

        // Act
        run_unsubscribe(&ctx, 100, 10).await;

        // Assert
        assert!(runner_stopped(&mut ctx.runner_stop_receivers[0]));
        assert!(!runner_stopped(&mut ctx.runner_stop_receivers[1]));
        assert!(ctx.upstream.recorded.unsubscribed_request_ids().is_empty());
        assert!(ctx.upstream.recorded.ingests_stopped().is_empty());
    }

    #[tokio::test]
    async fn publish_origin_keeps_upstream_subscription() {
        // Arrange
        let mut ctx = setup(ProducerOrigin::Publish, &[(100, 10)]).await;

        // Act
        run_unsubscribe(&ctx, 100, 10).await;

        // Assert
        assert!(runner_stopped(&mut ctx.runner_stop_receivers[0]));
        assert!(ctx.upstream.recorded.unsubscribed_request_ids().is_empty());
        assert!(ctx.upstream.recorded.ingests_stopped().is_empty());
    }

    #[tokio::test]
    async fn publish_origin_is_released_by_its_last_subscriber_on_a_draining_relay() {
        // Arrange
        let ctx = setup(ProducerOrigin::Publish, &[(100, 10)]).await;
        ctx.upstream.table.begin_draining(1);

        // Act
        run_unsubscribe(&ctx, 100, 10).await;

        // Assert
        assert_eq!(
            ctx.upstream.recorded.unsubscribed_request_ids(),
            vec![UPSTREAM_REQUEST_ID]
        );
        ctx.upstream
            .recorded
            .wait_ingest_stopped(UPSTREAM_REQUEST_ID)
            .await;
    }

    #[tokio::test]
    async fn the_last_subscriber_of_a_draining_relay_withdraws_the_route_of_the_released_publish() {
        // Arrange
        let ctx = setup(ProducerOrigin::Publish, &[(100, 10)]).await;
        ctx.namespace_routes
            .reconcile_track(&track_key(), &ctx.upstream.table)
            .await
            .unwrap();
        ctx.upstream.table.begin_draining(1);

        // Act
        run_unsubscribe(&ctx, 100, 10).await;

        // Assert
        assert!(!ctx.upstream.table.has_client_published_track(&track_key()));
        assert_eq!(
            ctx.route_registry.unregistered_track_publishers(),
            vec![track_key()]
        );
    }

    #[tokio::test]
    async fn a_released_subscribe_leaves_the_route_registry_alone() {
        // Arrange
        let ctx = setup(ProducerOrigin::Subscribe, &[(100, 10)]).await;

        // Act
        run_unsubscribe(&ctx, 100, 10).await;

        // Assert: the relay registers a route only for a client's PUBLISH
        assert!(
            ctx.route_registry
                .unregistered_track_publishers()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn unknown_subscription_stops_no_runner_and_forwards_nothing() {
        // Arrange
        let mut ctx = setup(ProducerOrigin::Subscribe, &[(101, 11)]).await;

        // Act
        run_unsubscribe(&ctx, 100, 10).await;

        // Assert
        assert!(!runner_stopped(&mut ctx.runner_stop_receivers[0]));
        assert!(ctx.upstream.recorded.unsubscribed_request_ids().is_empty());
        assert!(ctx.upstream.recorded.ingests_stopped().is_empty());
    }
}
