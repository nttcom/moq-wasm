use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    relay::ingress::ingress_coordinator::IngressCommand,
    sequences::tables::table::{LocalPubSubDirectory, UpstreamSubscriptionKey},
    types::{SessionId, TrackKey},
};
use tracing::Span;

/// §2.5: a subscriber detecting a Malformed Track MUST UNSUBSCRIBE it.
/// In-flight upstream fetches FETCH_CANCEL themselves in `FetchIngest`.
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
        table: &dyn LocalPubSubDirectory,
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

        if let Err(err) = forwarder
            .unsubscribe(publisher_session_id, removed.upstream_request_id)
            .await
        {
            tracing::warn!(
                ?err,
                request_id = %removed.upstream_request_id,
                "failed to send upstream unsubscribe for malformed track"
            );
        } else {
            tracing::info!(
                request_id = %removed.upstream_request_id,
                "sent upstream unsubscribe for malformed track"
            );
        }

        if ingress_sender
            .send(IngressCommand::StopTrack {
                track_key: track_key.clone(),
                publisher_session_id,
            })
            .await
            .is_err()
        {
            tracing::error!("failed to send ingress stop request");
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::*;
    use crate::modules::{
        core::mocks::{RecordedControlMessages, session_repository_with_mock_control_session},
        enums::ContentExists,
        sequences::tables::{
            hashmap_table::InMemoryLocalPubSubDirectory,
            table::{ActiveUpstreamSubscription, UpstreamSubscriptionOrigin},
        },
    };

    const PUBLISHER_SESSION: SessionId = 1;
    const UPSTREAM_REQUEST_ID: u64 = 42;

    struct TestContext {
        table: InMemoryLocalPubSubDirectory,
        forwarder: ControlMessageForwarder,
        ingress_sender: mpsc::Sender<IngressCommand>,
        ingress_receiver: mpsc::Receiver<IngressCommand>,
        recorded: RecordedControlMessages,
        track_key: TrackKey,
    }

    async fn setup() -> TestContext {
        let track_key = TrackKey::new("ns", "track");
        let table = InMemoryLocalPubSubDirectory::new();
        let upstream_key = UpstreamSubscriptionKey {
            publisher_session_id: PUBLISHER_SESSION,
            track_namespace: track_key.track_namespace.clone(),
            track_name: track_key.track_name.clone(),
        };
        table.register_upstream_subscription(
            upstream_key,
            ActiveUpstreamSubscription {
                upstream_request_id: UPSTREAM_REQUEST_ID,
                track_key: track_key.clone(),
                expires: None,
                content_exists: ContentExists::False,
                downstream_subscriber_count: 1,
                origin: UpstreamSubscriptionOrigin::Subscribe,
            },
        );

        let (repository, recorded) =
            session_repository_with_mock_control_session(PUBLISHER_SESSION).await;
        let forwarder = ControlMessageForwarder { repository };

        let (ingress_sender, ingress_receiver) = mpsc::channel(8);
        TestContext {
            table,
            forwarder,
            ingress_sender,
            ingress_receiver,
            recorded,
            track_key,
        }
    }

    async fn run_cleanup(ctx: &TestContext) {
        MalformedTrackCleanup
            .handle(
                PUBLISHER_SESSION,
                &tracing::Span::none(),
                &ctx.track_key,
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
        assert!(
            ctx.table
                .get_active_upstream_subscription(PUBLISHER_SESSION, "ns", "track")
                .is_none()
        );
        assert_eq!(
            *ctx.recorded.unsubscribed_request_ids.lock().unwrap(),
            vec![UPSTREAM_REQUEST_ID]
        );
        match ctx.ingress_receiver.try_recv() {
            Ok(IngressCommand::StopTrack {
                track_key,
                publisher_session_id,
            }) => {
                assert_eq!(track_key, ctx.track_key);
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
