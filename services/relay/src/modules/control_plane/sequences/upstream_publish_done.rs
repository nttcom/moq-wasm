use crate::modules::{
    cascading::route_registry::RelayRouteRegistry,
    control_plane::sequences::publish::Publish,
    data_plane::cache::store::TrackCacheStore,
    directory::InMemoryLocalPubSubDirectory,
    domain::{publish_done_reason::PublishDoneReason, session_id::SessionId},
};

pub(crate) struct UpstreamPublishDone;

impl UpstreamPublishDone {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle(
        &self,
        publisher_session_id: SessionId,
        upstream_request_id: u64,
        end: PublishDoneReason,
        table: &InMemoryLocalPubSubDirectory,
        cache_store: &TrackCacheStore,
        route_registry: &dyn RelayRouteRegistry,
    ) {
        let Some(track_key) =
            table.end_upstream_subscription(publisher_session_id, upstream_request_id, end)
        else {
            tracing::debug!("PUBLISH_DONE for no active upstream subscription");
            return;
        };
        if !table.has_track_publisher(&track_key) {
            cache_store.publisher_left(&track_key);
        }
        Publish::withdraw_track_route(table, route_registry, &track_key).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::test_support::directory_fixtures::delivery_control;
    use crate::modules::{
        cascading::route_registry::NoopRelayRouteRegistry,
        directory::track::ProducerOrigin,
        domain::session_peer::SessionPeer,
        test_support::directory_fixtures::{
            PUBLISHER_SESSION, UPSTREAM_REQUEST_ID, UpstreamReleaseContext, track_key,
            upstream_release_context,
        },
    };
    use moqt::fetch::JoinPoint;

    const DOWNSTREAM_SESSION: SessionId = 2;
    const DOWNSTREAM_SUBSCRIBE_ID: u64 = 100;

    async fn receive_publish_done(ctx: &UpstreamReleaseContext, upstream_request_id: u64) {
        UpstreamPublishDone
            .handle(
                PUBLISHER_SESSION,
                upstream_request_id,
                PublishDoneReason::publisher_session_closed(),
                &ctx.table,
                &TrackCacheStore::new(),
                &NoopRelayRouteRegistry,
            )
            .await;
    }

    #[tokio::test]
    async fn publish_done_ends_the_downstream_subscriptions_and_stops_ingress() {
        // Arrange
        let ctx = upstream_release_context(ProducerOrigin::Subscribe).await;
        let mut runner_stop_receiver = ctx
            .table
            .register_downstream_subscription(
                DOWNSTREAM_SESSION,
                DOWNSTREAM_SUBSCRIBE_ID,
                SessionPeer::Client,
                track_key(),
                JoinPoint::Largest(None),
                delivery_control(true),
            )
            .unwrap()
            .stop_receiver;

        // Act
        receive_publish_done(&ctx, UPSTREAM_REQUEST_ID).await;

        // Assert
        assert_eq!(
            runner_stop_receiver.try_recv(),
            Ok(PublishDoneReason::publisher_session_closed())
        );
        assert!(ctx.table.upstream_tracks.is_empty());
        assert!(ctx.table.downstream_subscription_states().is_empty());
        assert!(ctx.recorded.unsubscribed_request_ids().is_empty());
        ctx.recorded.wait_ingest_stopped(UPSTREAM_REQUEST_ID).await;
    }

    #[tokio::test]
    async fn publish_done_for_another_request_changes_nothing() {
        // Arrange
        let ctx = upstream_release_context(ProducerOrigin::Subscribe).await;

        // Act
        receive_publish_done(&ctx, UPSTREAM_REQUEST_ID + 1).await;

        // Assert
        assert_eq!(ctx.table.upstream_tracks.len(), 1);
        assert!(ctx.recorded.ingests_stopped().is_empty());
    }
}
