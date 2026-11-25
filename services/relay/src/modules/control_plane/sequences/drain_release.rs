use crate::modules::{
    cascading::namespace_routes::NamespaceRoutes,
    control_plane::control_message_forwarder::ControlMessageForwarder,
    directory::InMemoryLocalPubSubDirectory, domain::session_id::SessionId,
};

/// Unsubscribes the tracks a publisher announced with PUBLISH that nobody on
/// this draining relay subscribes to, so a publisher that also publishes on
/// the relay named by GOAWAY stops sending them here (draft-22 §7.6.1). The
/// tracks still watched are released when their last subscriber leaves.
pub(crate) struct ReleaseUnwatchedPublishes;

impl ReleaseUnwatchedPublishes {
    pub(crate) async fn handle(
        &self,
        publisher_session_id: SessionId,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        namespace_routes: &NamespaceRoutes,
    ) {
        for (track_key, released) in
            table.release_unwatched_published_subscriptions(publisher_session_id)
        {
            super::release_upstream_subscriptions(
                table,
                forwarder,
                namespace_routes,
                &track_key,
                std::slice::from_ref(&released),
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {

    use std::sync::Arc;

    use super::*;
    use crate::modules::test_support::directory_fixtures::downstream_subscription;
    use crate::modules::{
        directory::track::ProducerOrigin,
        domain::session_peer::SessionPeer,
        test_support::{
            directory_fixtures::{
                PUBLISHER_SESSION, UPSTREAM_REQUEST_ID, UpstreamReleaseContext, delivery_control,
                register_downstream, track_key, upstream_release_context,
            },
            stub_route_registry::{PublisherLookup, StubRouteRegistry},
        },
    };

    async fn release(ctx: &UpstreamReleaseContext) {
        release_with(ctx, &NamespaceRoutes::unregistered()).await;
    }

    async fn release_with(ctx: &UpstreamReleaseContext, namespace_routes: &NamespaceRoutes) {
        ReleaseUnwatchedPublishes
            .handle(
                PUBLISHER_SESSION,
                &ctx.table,
                &ctx.forwarder,
                namespace_routes,
            )
            .await;
    }

    #[tokio::test]
    async fn a_published_track_nobody_watches_is_unsubscribed() {
        // Arrange
        let ctx = upstream_release_context(ProducerOrigin::Publish).await;

        // Act
        release(&ctx).await;

        // Assert
        assert_eq!(
            ctx.recorded.unsubscribed_request_ids(),
            vec![UPSTREAM_REQUEST_ID]
        );
        ctx.recorded.wait_ingest_stopped(UPSTREAM_REQUEST_ID).await;
        assert!(ctx.table.upstream_tracks.get(&track_key()).is_none());
    }

    #[tokio::test]
    async fn an_unwatched_published_track_has_its_route_withdrawn() {
        // Arrange
        let ctx = upstream_release_context(ProducerOrigin::Publish).await;
        let route_registry = Arc::new(StubRouteRegistry::new(PublisherLookup::MustNotBeCalled));
        let namespace_routes = NamespaceRoutes::new(route_registry.clone());
        namespace_routes
            .reconcile_track(&track_key(), &ctx.table)
            .await
            .unwrap();

        // Act
        release_with(&ctx, &namespace_routes).await;

        // Assert
        assert!(!ctx.table.has_client_published_track(&track_key()));
        assert_eq!(
            route_registry.unregistered_track_publishers(),
            vec![track_key()]
        );
    }

    #[tokio::test]
    async fn a_watched_published_track_stays_until_its_last_subscriber_leaves() {
        // Arrange
        let ctx = upstream_release_context(ProducerOrigin::Publish).await;
        let _registered = register_downstream(
            &ctx.table,
            100,
            10,
            SessionPeer::Client,
            downstream_subscription(track_key(), None),
            delivery_control(true),
        )
        .unwrap();

        // Act
        release(&ctx).await;

        // Assert
        assert!(ctx.recorded.unsubscribed_request_ids().is_empty());
        assert!(ctx.recorded.ingests_stopped().is_empty());
    }

    #[tokio::test]
    async fn a_track_the_relay_subscribed_to_is_left_to_its_subscribers() {
        // Arrange
        let ctx = upstream_release_context(ProducerOrigin::Subscribe).await;

        // Act
        release(&ctx).await;

        // Assert
        assert!(ctx.recorded.unsubscribed_request_ids().is_empty());
        assert!(ctx.recorded.ingests_stopped().is_empty());
    }
}
