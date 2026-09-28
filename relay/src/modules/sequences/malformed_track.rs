use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    relay::ingress::ingress_coordinator::IngressCommand,
    sequences::tables::{
        hashmap_table::InMemoryLocalPubSubDirectory, table::UpstreamSubscriptionKey,
    },
    types::{SessionId, TrackKey},
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
        let upstream_key = UpstreamSubscriptionKey {
            publisher_session_id,
            track_namespace: track_key.track_namespace.clone(),
            track_name: track_key.track_name.clone(),
        };
        let Some(removed) = table.remove_upstream_subscription(&upstream_key) else {
            tracing::debug!("upstream subscription already removed");
            return;
        };

        super::release_upstream(
            forwarder,
            ingress_sender,
            publisher_session_id,
            removed.upstream_request_id,
            track_key,
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::sequences::{
        tables::table::UpstreamSubscriptionOrigin,
        test_fixtures::{
            PUBLISHER_SESSION, UPSTREAM_REQUEST_ID, UpstreamReleaseContext,
            upstream_release_context,
        },
    };

    async fn setup() -> UpstreamReleaseContext {
        upstream_release_context(UpstreamSubscriptionOrigin::Subscribe).await
    }

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
        let mut ctx = setup().await;

        // Act
        run_cleanup(&ctx).await;

        // Assert
        assert!(ctx.table.active_upstream_subscriptions.is_empty());
        assert_eq!(
            *ctx.recorded.unsubscribed_request_ids.lock().unwrap(),
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
        let mut ctx = setup().await;
        run_cleanup(&ctx).await;
        let _ = ctx.ingress_receiver.try_recv();

        // Act: a second reader reports the same detection.
        run_cleanup(&ctx).await;

        // Assert
        assert_eq!(
            *ctx.recorded.unsubscribed_request_ids.lock().unwrap(),
            vec![UPSTREAM_REQUEST_ID]
        );
        assert!(ctx.ingress_receiver.try_recv().is_err());
    }
}
