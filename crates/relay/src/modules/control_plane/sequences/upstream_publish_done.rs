use crate::modules::{
    data_plane::ingress::ingress_coordinator::IngressCommand,
    domain::{
        pub_sub_directory::{InMemoryLocalPubSubDirectory, entry::PublishDoneReason},
        session_id::SessionId,
    },
};
use tracing::Span;

pub(crate) struct UpstreamPublishDone;

impl UpstreamPublishDone {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.upstream_publish_done",
        skip_all,
        parent = session_span,
        fields(
            publisher_session_id = %publisher_session_id,
            request_id = upstream_request_id,
            status_code = end.status_code,
        )
    )]
    pub(crate) async fn handle(
        &self,
        publisher_session_id: SessionId,
        session_span: &Span,
        upstream_request_id: u64,
        end: PublishDoneReason,
        table: &InMemoryLocalPubSubDirectory,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
    ) {
        let Some(track_key) =
            table.end_upstream_subscription(publisher_session_id, upstream_request_id, end)
        else {
            tracing::debug!("PUBLISH_DONE for no active upstream subscription");
            return;
        };
        super::stop_ingress(ingress_sender, publisher_session_id, &track_key).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{
        domain::{pub_sub_directory::entry::UpstreamSubscriptionOrigin, track_key::TrackKey},
        test_support::directory_fixtures::{
            PUBLISHER_SESSION, UPSTREAM_REQUEST_ID, UpstreamReleaseContext, upstream_key,
            upstream_release_context,
        },
    };

    const DOWNSTREAM_SESSION: SessionId = 2;
    const DOWNSTREAM_SUBSCRIBE_ID: u64 = 100;

    async fn receive_publish_done(ctx: &UpstreamReleaseContext, upstream_request_id: u64) {
        UpstreamPublishDone
            .handle(
                PUBLISHER_SESSION,
                &Span::none(),
                upstream_request_id,
                PublishDoneReason::publisher_session_closed(),
                &ctx.table,
                &ctx.ingress_sender,
            )
            .await;
    }

    #[tokio::test]
    async fn publish_done_ends_the_downstream_subscriptions_and_stops_ingress() {
        // Arrange
        let mut ctx = upstream_release_context(UpstreamSubscriptionOrigin::Subscribe).await;
        let mut runner_stop_receiver = ctx
            .table
            .register_downstream_subscription(
                DOWNSTREAM_SESSION,
                DOWNSTREAM_SUBSCRIBE_ID,
                upstream_key(),
                None,
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
        assert!(ctx.table.active_upstream_subscriptions.is_empty());
        assert!(ctx.table.downstream_subscriptions.is_empty());
        assert!(ctx.recorded.unsubscribed_request_ids().is_empty());
        assert!(matches!(
            ctx.ingress_receiver.try_recv(),
            Ok(IngressCommand::StopTrack { track_key, publisher_session_id })
                if track_key == TrackKey::new("ns", "track")
                    && publisher_session_id == PUBLISHER_SESSION
        ));
    }

    #[tokio::test]
    async fn publish_done_for_another_request_changes_nothing() {
        // Arrange
        let mut ctx = upstream_release_context(UpstreamSubscriptionOrigin::Subscribe).await;

        // Act
        receive_publish_done(&ctx, UPSTREAM_REQUEST_ID + 1).await;

        // Assert
        assert_eq!(ctx.table.active_upstream_subscriptions.len(), 1);
        assert!(ctx.ingress_receiver.try_recv().is_err());
    }
}
