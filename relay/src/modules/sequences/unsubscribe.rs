use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    core::handler::unsubscribe::UnsubscribeHandler,
    relay::ingress::ingress_coordinator::IngressCommand,
    sequences::tables::{
        hashmap_table::InMemoryLocalPubSubDirectory, table::UpstreamSubscriptionOrigin,
    },
    types::SessionId,
};
use tracing::Span;

pub(crate) struct Unsubscribe;

impl Unsubscribe {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.unsubscribe",
        skip_all,
        parent = session_span,
        fields(session_id = %session_id)
    )]
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
        handler: Box<dyn UnsubscribeHandler>,
    ) {
        let subscribe_id = handler.subscribe_id();
        tracing::info!(
            session_id = %session_id,
            subscribe_id = %subscribe_id,
            "SequenceHandler::unsubscribe"
        );

        let Some(removed) = table.remove_downstream_subscription(session_id, subscribe_id) else {
            tracing::warn!(
                session_id = %session_id,
                subscribe_id = %subscribe_id,
                "active downstream subscription not found"
            );
            return;
        };

        tracing::info!(
            session_id = %session_id,
            subscribe_id = %subscribe_id,
            upstream_session_id = %removed.upstream_key.publisher_session_id,
            track_namespace = %removed.upstream_key.track_namespace,
            track_name = %removed.upstream_key.track_name,
            remaining_downstream_subscriber_count = removed.remaining_downstream_subscriber_count,
            "downstream unsubscribe processed"
        );

        if removed.remaining_downstream_subscriber_count == 0
            && removed.upstream_origin == UpstreamSubscriptionOrigin::Subscribe
        {
            super::release_upstream(
                forwarder,
                ingress_sender,
                removed.upstream_key.publisher_session_id,
                removed.upstream_request_id,
                &removed.track_key,
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::oneshot;

    use super::*;
    use crate::modules::{
        core::mocks::runner_stopped,
        sequences::test_fixtures::{
            PUBLISHER_SESSION, UPSTREAM_REQUEST_ID, UpstreamReleaseContext,
            upstream_release_context,
        },
        types::TrackKey,
    };

    struct MockUnsubscribeHandler {
        subscribe_id: u64,
    }

    impl UnsubscribeHandler for MockUnsubscribeHandler {
        fn subscribe_id(&self) -> u64 {
            self.subscribe_id
        }
    }

    struct TestContext {
        upstream: UpstreamReleaseContext,
        runner_stop_receivers: Vec<oneshot::Receiver<()>>,
    }

    async fn setup(
        origin: UpstreamSubscriptionOrigin,
        downstream_subscriptions: &[(SessionId, u64)],
    ) -> TestContext {
        let upstream = upstream_release_context(origin).await;
        let runner_stop_receivers = downstream_subscriptions
            .iter()
            .map(|(session_id, subscribe_id)| {
                upstream
                    .table
                    .register_downstream_subscription(
                        *session_id,
                        *subscribe_id,
                        upstream.upstream_key.clone(),
                        None,
                    )
                    .unwrap()
            })
            .collect();
        TestContext {
            upstream,
            runner_stop_receivers,
        }
    }

    async fn run_unsubscribe(ctx: &TestContext, session_id: SessionId, subscribe_id: u64) {
        Unsubscribe
            .handle(
                session_id,
                &tracing::Span::none(),
                &ctx.upstream.table,
                &ctx.upstream.forwarder,
                &ctx.upstream.ingress_sender,
                Box::new(MockUnsubscribeHandler { subscribe_id }),
            )
            .await;
    }

    #[tokio::test]
    async fn last_subscriber_forwards_upstream_unsubscribe_and_stops_ingress() {
        // Arrange
        let mut ctx = setup(UpstreamSubscriptionOrigin::Subscribe, &[(100, 10)]).await;

        // Act
        run_unsubscribe(&ctx, 100, 10).await;

        // Assert
        assert!(runner_stopped(&mut ctx.runner_stop_receivers[0]));
        assert_eq!(
            *ctx.upstream
                .recorded
                .unsubscribed_request_ids
                .lock()
                .unwrap(),
            vec![UPSTREAM_REQUEST_ID]
        );
        match ctx.upstream.ingress_receiver.try_recv() {
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
    async fn remaining_subscribers_keep_upstream_subscription() {
        // Arrange
        let mut ctx = setup(
            UpstreamSubscriptionOrigin::Subscribe,
            &[(100, 10), (101, 11)],
        )
        .await;

        // Act
        run_unsubscribe(&ctx, 100, 10).await;

        // Assert
        assert!(runner_stopped(&mut ctx.runner_stop_receivers[0]));
        assert!(!runner_stopped(&mut ctx.runner_stop_receivers[1]));
        assert!(
            ctx.upstream
                .recorded
                .unsubscribed_request_ids
                .lock()
                .unwrap()
                .is_empty()
        );
        assert!(ctx.upstream.ingress_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn publish_origin_keeps_upstream_subscription() {
        // Arrange
        let mut ctx = setup(UpstreamSubscriptionOrigin::Publish, &[(100, 10)]).await;

        // Act
        run_unsubscribe(&ctx, 100, 10).await;

        // Assert
        assert!(runner_stopped(&mut ctx.runner_stop_receivers[0]));
        assert!(
            ctx.upstream
                .recorded
                .unsubscribed_request_ids
                .lock()
                .unwrap()
                .is_empty()
        );
        assert!(ctx.upstream.ingress_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn unknown_subscription_stops_no_runner_and_forwards_nothing() {
        // Arrange
        let mut ctx = setup(UpstreamSubscriptionOrigin::Subscribe, &[(101, 11)]).await;

        // Act
        run_unsubscribe(&ctx, 100, 10).await;

        // Assert
        assert!(!runner_stopped(&mut ctx.runner_stop_receivers[0]));
        assert!(
            ctx.upstream
                .recorded
                .unsubscribed_request_ids
                .lock()
                .unwrap()
                .is_empty()
        );
        assert!(ctx.upstream.ingress_receiver.try_recv().is_err());
    }
}
