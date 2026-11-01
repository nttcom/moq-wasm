use crate::modules::{
    control_plane::control_message_forwarder::ControlMessageForwarder,
    directory::InMemoryLocalPubSubDirectory, domain::session_id::SessionId,
};

pub(crate) struct Unsubscribe;

impl Unsubscribe {
    /// Removes the downstream subscription `subscribe_id` and releases the
    /// upstream subscriptions it leaves unneeded; runs for an UNSUBSCRIBE and
    /// for a subscription whose egress runner delivered its whole range.
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        subscribe_id: u64,
    ) {
        tracing::debug!(
            session_id = %session_id,
            request_id = %subscribe_id,
            "UNSUBSCRIBE received"
        );

        let Some(removed) = table.remove_downstream_subscription(session_id, subscribe_id) else {
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

        for released in removed.released_upstream_subscriptions {
            super::release_upstream(
                forwarder,
                released.publisher_session_id,
                &released.subscription,
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::modules::test_support::directory_fixtures::delivery_control;
    use moqt::fetch::JoinPoint;
    use tokio::sync::oneshot;

    use super::*;
    use crate::modules::{
        directory::track::{ProducerOrigin, PublishDoneReason},
        domain::session_peer::SessionPeer,
        test_support::{
            directory_fixtures::{
                UPSTREAM_REQUEST_ID, UpstreamReleaseContext, track_key, upstream_release_context,
            },
            mock_session::runner_stopped,
        },
    };

    struct TestContext {
        upstream: UpstreamReleaseContext,
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
                upstream
                    .table
                    .register_downstream_subscription(
                        *session_id,
                        *subscribe_id,
                        SessionPeer::Client,
                        track_key(),
                        JoinPoint::Largest(None),
                        delivery_control(true),
                    )
                    .unwrap()
                    .stop_receiver
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
                &ctx.upstream.table,
                &ctx.upstream.forwarder,
                subscribe_id,
            )
            .await;
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
