use crate::modules::{
    control_plane::control_message_forwarder::ControlMessageForwarder,
    data_plane::ingress::ingress_coordinator::IngressCommand,
    domain::{
        pub_sub_directory::InMemoryLocalPubSubDirectory, session_id::SessionId, track_key::TrackKey,
    },
};
use tracing::Span;

pub(crate) struct MalformedTrackCleanup;

impl MalformedTrackCleanup {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.malformed_track_cleanup",
        skip_all,
        parent = session_span,
        fields(publisher_session_id = %publisher_session_id, track_key = %track_key)
    )]
    pub(crate) async fn handle(
        &self,
        publisher_session_id: SessionId,
        session_span: &Span,
        track_key: &TrackKey,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
    ) {
        let released = table.remove_upstream_track(track_key);
        // draft-14 §2.5: the cache latch makes the whole track malformed, so
        // every publisher's subscription is ended, not only the reporting one.
        for (publisher_session_id, subscription) in released {
            super::release_upstream(
                forwarder,
                ingress_sender,
                publisher_session_id,
                subscription.upstream_request_id,
                track_key,
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{
        domain::pub_sub_directory::entry::UpstreamSubscriptionOrigin,
        test_support::directory_fixtures::{
            PUBLISHER_SESSION, UPSTREAM_REQUEST_ID, UpstreamReleaseContext, active_upstream,
            upstream_release_context,
        },
    };

    async fn run_cleanup(ctx: &UpstreamReleaseContext) {
        MalformedTrackCleanup
            .handle(
                PUBLISHER_SESSION,
                &tracing::Span::none(),
                &TrackKey::new("ns", "track"),
                &ctx.table,
                &ctx.forwarder,
                &ctx.ingress_sender,
            )
            .await;
    }

    #[tokio::test]
    async fn detection_unsubscribes_upstream_and_stops_ingress() {
        // Arrange
        let mut ctx = upstream_release_context(UpstreamSubscriptionOrigin::Subscribe).await;

        // Act
        run_cleanup(&ctx).await;

        // Assert
        assert!(ctx.table.upstream_tracks.is_empty());
        assert_eq!(
            ctx.recorded.unsubscribed_request_ids(),
            vec![UPSTREAM_REQUEST_ID]
        );
        match ctx.ingress_receiver.try_recv() {
            Ok(IngressCommand::StopTrack {
                track_key,
                publisher_session_id,
            }) => {
                assert_eq!(track_key, TrackKey::new("ns", "track"));
                assert_eq!(publisher_session_id, PUBLISHER_SESSION);
            }
            other => panic!("Expected StopTrack, got {:?}", other.is_ok()),
        }
    }

    #[tokio::test]
    async fn duplicate_detection_reports_are_idempotent() {
        // Arrange: the first report already ran the cleanup.
        let mut ctx = upstream_release_context(UpstreamSubscriptionOrigin::Subscribe).await;
        run_cleanup(&ctx).await;
        let _ = ctx.ingress_receiver.try_recv();

        // Act: a second reader reports the same detection.
        run_cleanup(&ctx).await;

        // Assert
        assert_eq!(
            ctx.recorded.unsubscribed_request_ids(),
            vec![UPSTREAM_REQUEST_ID]
        );
        assert!(ctx.ingress_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn detection_ends_the_upstream_subscription_of_every_publisher() {
        // Arrange
        const OTHER_PUBLISHER_SESSION: SessionId = 3;
        let mut ctx = upstream_release_context(UpstreamSubscriptionOrigin::Subscribe).await;
        ctx.table.register_upstream_subscription(
            TrackKey::new("ns", "track"),
            OTHER_PUBLISHER_SESSION,
            active_upstream(UpstreamSubscriptionOrigin::Subscribe),
        );

        // Act
        run_cleanup(&ctx).await;

        // Assert
        assert!(ctx.table.upstream_tracks.is_empty());
        let mut stopped_publishers = Vec::new();
        while let Ok(IngressCommand::StopTrack {
            publisher_session_id,
            ..
        }) = ctx.ingress_receiver.try_recv()
        {
            stopped_publishers.push(publisher_session_id);
        }
        stopped_publishers.sort();
        assert_eq!(
            stopped_publishers,
            vec![PUBLISHER_SESSION, OTHER_PUBLISHER_SESSION]
        );
    }
}
