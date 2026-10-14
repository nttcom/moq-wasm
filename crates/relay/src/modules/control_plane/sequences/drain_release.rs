use crate::modules::{
    control_plane::control_message_forwarder::ControlMessageForwarder,
    data_plane::ingress::ingress_coordinator::IngressCommand,
    domain::{pub_sub_directory::InMemoryLocalPubSubDirectory, session_id::SessionId},
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
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
    ) {
        for (track_key, released) in
            table.release_unwatched_published_subscriptions(publisher_session_id)
        {
            super::release_upstream(
                forwarder,
                ingress_sender,
                publisher_session_id,
                &released.subscription,
                &track_key,
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{
        domain::{pub_sub_directory::entry::UpstreamSubscriptionOrigin, session_peer::SessionPeer},
        test_support::directory_fixtures::{
            PUBLISHER_SESSION, UPSTREAM_REQUEST_ID, UpstreamReleaseContext, track_key,
            upstream_release_context,
        },
    };

    async fn release(ctx: &UpstreamReleaseContext) {
        ReleaseUnwatchedPublishes
            .handle(
                PUBLISHER_SESSION,
                &ctx.table,
                &ctx.forwarder,
                &ctx.ingress_sender,
            )
            .await;
    }

    #[tokio::test]
    async fn a_published_track_nobody_watches_is_unsubscribed() {
        // Arrange
        let mut ctx = upstream_release_context(UpstreamSubscriptionOrigin::Publish).await;

        // Act
        release(&ctx).await;

        // Assert
        assert_eq!(
            ctx.recorded.unsubscribed_request_ids(),
            vec![UPSTREAM_REQUEST_ID]
        );
        assert!(matches!(
            ctx.ingress_receiver.try_recv(),
            Ok(IngressCommand::StopTrack { .. })
        ));
        assert!(ctx.table.upstream_tracks.get(&track_key()).is_none());
    }

    #[tokio::test]
    async fn a_watched_published_track_stays_until_its_last_subscriber_leaves() {
        // Arrange
        let mut ctx = upstream_release_context(UpstreamSubscriptionOrigin::Publish).await;
        let _registered = ctx
            .table
            .register_downstream_subscription(100, 10, SessionPeer::Client, track_key(), None, true)
            .unwrap();

        // Act
        release(&ctx).await;

        // Assert
        assert!(ctx.recorded.unsubscribed_request_ids().is_empty());
        assert!(ctx.ingress_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_track_the_relay_subscribed_to_is_left_to_its_subscribers() {
        // Arrange
        let mut ctx = upstream_release_context(UpstreamSubscriptionOrigin::Subscribe).await;

        // Act
        release(&ctx).await;

        // Assert
        assert!(ctx.recorded.unsubscribed_request_ids().is_empty());
        assert!(ctx.ingress_receiver.try_recv().is_err());
    }
}
