use anyhow::bail;
use moqt::{ContentExists, wire::publish_done_status_code};
use tokio::sync::{mpsc, oneshot};
use tracing::Span;

use crate::modules::{
    control_plane::{
        control_message_forwarder::ControlMessageForwarder, sequences::subscribe::cached_largest,
    },
    data_plane::{
        cache::store::TrackCacheStore,
        egress::coordinator::{EgressCommand, EgressStartRequest},
    },
    domain::{
        pub_sub_directory::{InMemoryLocalPubSubDirectory, entry::PublishDoneReason},
        session_id::SessionId,
        track_key::TrackKey,
    },
};

pub(crate) struct DownstreamPublish<'a> {
    pub(crate) table: &'a InMemoryLocalPubSubDirectory,
    pub(crate) forwarder: &'a ControlMessageForwarder,
    pub(crate) egress_sender: &'a mpsc::Sender<EgressCommand>,
    pub(crate) cache_store: &'a TrackCacheStore,
}

impl DownstreamPublish<'_> {
    /// A subscriber that already receives the track, e.g. from an earlier
    /// publisher of it, gets no second PUBLISH.
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.downstream_publish",
        skip_all,
        fields(subscriber_session_id = %subscriber_session_id, track_key = %track_key)
    )]
    pub(crate) async fn send(
        &self,
        subscriber_session_id: SessionId,
        track_key: &TrackKey,
    ) -> anyhow::Result<()> {
        if self
            .table
            .has_downstream_subscription_to_track(subscriber_session_id, track_key)
        {
            return Ok(());
        }
        let Some(upstream_track) = self.table.get_upstream_track(track_key) else {
            bail!("upstream track ended before PUBLISH was sent");
        };
        let upstream_largest = match upstream_track.content_exists() {
            ContentExists::True { location } => Some(location),
            ContentExists::False => None,
        };
        let largest_location = cached_largest(self.cache_store, track_key).max(upstream_largest);
        let content_exists = largest_location.map_or(ContentExists::False, |location| {
            ContentExists::True { location }
        });

        let downstream_subscription = self
            .forwarder
            .publish(
                subscriber_session_id,
                track_key.track_namespace.clone(),
                track_key.track_name.clone(),
                content_exists,
            )
            .await?;
        let request_id = downstream_subscription.request_id();
        let Some(forward) = downstream_subscription.publish_ok_forward() else {
            bail!("PUBLISH answered without a publisher-initiated subscription");
        };
        let subscriber_peer = super::session_peer(subscriber_session_id, self.forwarder).await;

        let Some(runner_signals) = self.table.register_downstream_subscription(
            subscriber_session_id,
            request_id,
            subscriber_peer,
            track_key.clone(),
            largest_location,
            forward,
        ) else {
            let track_ended = PublishDoneReason {
                status_code: publish_done_status_code::TRACK_ENDED,
                error_reason: "track ended before PUBLISH_OK".to_string(),
            };
            return self
                .forwarder
                .publish_done(subscriber_session_id, request_id, track_ended)
                .await;
        };

        let (ready_sender, _ready_receiver) = oneshot::channel();
        let (publish_ok_sender, publish_ok_receiver) = oneshot::channel();
        let _ = publish_ok_sender.send(());
        if self
            .egress_sender
            .send(EgressCommand::StartReader(Box::new(EgressStartRequest {
                subscriber_session_id,
                downstream_subscribe_id: request_id,
                track_key: track_key.clone(),
                downstream_subscription,
                parent_span: Span::current(),
                ready_sender,
                runner_signals,
                subscribe_ok_receiver: publish_ok_receiver,
                largest_location,
            })))
            .await
            .is_err()
        {
            bail!("failed to send EgressStartRequest");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::modules::{
        domain::pub_sub_directory::entry::UpstreamSubscriptionOrigin,
        test_support::{
            directory_fixtures::{
                DownstreamPublishContext, NAMESPACE_SUBSCRIBER_SESSION, PUBLISHER_SESSION,
                downstream_publish_context, table_with_upstream, track_key,
            },
            mock_session::{PUBLISH_REQUEST_ID, SentPublish},
            relay_harness::fixtures::{cached_object::insert_closed_group, location},
        },
    };

    fn published_table() -> Arc<InMemoryLocalPubSubDirectory> {
        Arc::new(table_with_upstream(UpstreamSubscriptionOrigin::Publish).0)
    }

    async fn context_accepting_publish() -> DownstreamPublishContext {
        downstream_publish_context(published_table(), || Ok(true)).await
    }

    async fn send_publish(ctx: &DownstreamPublishContext) -> anyhow::Result<()> {
        ctx.downstream_publish()
            .send(NAMESPACE_SUBSCRIBER_SESSION, &track_key())
            .await
    }

    fn take_egress_start(ctx: &mut DownstreamPublishContext) -> EgressStartRequest {
        match ctx.egress_receiver.try_recv() {
            Ok(EgressCommand::StartReader(request)) => *request,
            _ => panic!("expected the forwarded PUBLISH to start an egress reader"),
        }
    }

    #[tokio::test]
    async fn publish_ok_registers_the_downstream_subscription_and_starts_its_egress() {
        // Arrange
        let mut ctx = context_accepting_publish().await;

        // Act
        send_publish(&ctx).await.unwrap();

        // Assert
        let registered = ctx
            .table
            .get_downstream_subscription(NAMESPACE_SUBSCRIBER_SESSION, PUBLISH_REQUEST_ID)
            .expect("PUBLISH_OK should register a downstream subscription");
        assert_eq!(registered.track_key, track_key());
        let egress_start = take_egress_start(&mut ctx);
        assert_eq!(
            egress_start.subscriber_session_id,
            NAMESPACE_SUBSCRIBER_SESSION
        );
        assert_eq!(egress_start.downstream_subscribe_id, PUBLISH_REQUEST_ID);
        assert_eq!(egress_start.track_key, track_key());
    }

    #[tokio::test]
    async fn publish_advertises_the_largest_location_egress_starts_after() {
        // Arrange
        let mut ctx = context_accepting_publish().await;
        let cache = ctx.cache_store.get_or_create(&track_key());
        insert_closed_group(&cache, 4, &[0, 1]);

        // Act
        send_publish(&ctx).await.unwrap();

        // Assert
        assert_eq!(
            ctx.subscriber.publishes(),
            vec![SentPublish {
                track_namespace: "ns".to_string(),
                track_name: "track".to_string(),
                content_exists: ContentExists::True {
                    location: location(4, 1)
                },
            }]
        );
        assert_eq!(
            take_egress_start(&mut ctx).largest_location,
            Some(location(4, 1))
        );
    }

    #[tokio::test]
    async fn egress_starts_with_the_forward_state_of_the_publish_ok() {
        // Arrange
        let mut ctx = downstream_publish_context(published_table(), || Ok(false)).await;

        // Act
        send_publish(&ctx).await.unwrap();

        // Assert
        assert!(
            !*take_egress_start(&mut ctx)
                .runner_signals
                .forward_receiver
                .borrow()
        );
    }

    #[tokio::test]
    async fn publisher_session_cleanup_ends_the_forwarded_publish_with_publish_done() {
        // Arrange
        let mut ctx = context_accepting_publish().await;
        send_publish(&ctx).await.unwrap();
        let mut egress_start = take_egress_start(&mut ctx);

        // Act
        ctx.table.remove_session(PUBLISHER_SESSION);

        // Assert: the runner sends PUBLISH_DONE once stopped with a reason after its subscription was acknowledged
        assert_eq!(
            egress_start.runner_signals.stop_receiver.try_recv(),
            Ok(PublishDoneReason::publisher_session_closed())
        );
        assert_eq!(egress_start.subscribe_ok_receiver.try_recv(), Ok(()));
    }

    #[tokio::test]
    async fn publish_ok_after_the_upstream_ended_is_answered_with_publish_done() {
        // Arrange
        let table = published_table();
        let ended_table = table.clone();
        let mut ctx = downstream_publish_context(table, move || {
            ended_table.remove_session(PUBLISHER_SESSION);
            Ok(true)
        })
        .await;

        // Act
        send_publish(&ctx).await.unwrap();

        // Assert
        assert_eq!(
            ctx.subscriber.publish_dones(),
            vec![(PUBLISH_REQUEST_ID, publish_done_status_code::TRACK_ENDED)]
        );
        assert!(ctx.table.downstream_subscriptions.is_empty());
        assert!(ctx.egress_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_subscriber_already_receiving_the_track_gets_no_second_publish() {
        // Arrange
        let mut ctx = context_accepting_publish().await;
        send_publish(&ctx).await.unwrap();
        take_egress_start(&mut ctx);

        // Act
        send_publish(&ctx).await.unwrap();

        // Assert
        assert_eq!(ctx.subscriber.publishes().len(), 1);
        assert!(ctx.egress_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn publish_error_registers_nothing() {
        // Arrange
        let mut ctx =
            downstream_publish_context(published_table(), || anyhow::bail!("PUBLISH_ERROR")).await;

        // Act
        let result = send_publish(&ctx).await;

        // Assert
        assert!(result.is_err());
        assert!(ctx.table.downstream_subscriptions.is_empty());
        assert!(ctx.egress_receiver.try_recv().is_err());
    }
}
