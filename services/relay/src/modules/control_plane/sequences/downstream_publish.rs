pub(crate) mod downstream_publish_task;

use std::sync::Arc;

use anyhow::bail;
use moqt::{object::ContentExists, publish::PublishDoneCode};
use tokio::sync::oneshot;

use crate::modules::directory::track::DownstreamSubscription;
use crate::modules::{
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        sequences::{EgressRequest, start_egress, subscribe::existing_track_largest},
    },
    data_plane::cache::store::TrackCacheStore,
    directory::InMemoryLocalPubSubDirectory,
    domain::{
        delivery_control::DeliveryControl, publish_done_reason::PublishDoneReason,
        session_id::SessionId, track_key::TrackKey,
    },
};

#[derive(Clone)]
pub(crate) struct DownstreamPublish {
    pub(crate) table: Arc<InMemoryLocalPubSubDirectory>,
    pub(crate) forwarder: ControlMessageForwarder,
    pub(crate) cache_store: Arc<TrackCacheStore>,
}

impl DownstreamPublish {
    /// A subscriber that already receives the track, e.g. from an earlier
    /// publisher of it, gets no second PUBLISH, nor does one whose
    /// subscription the track's PUBLISH was blocked on (draft-18 §6.1 MUST
    /// NOT). `forward` is the Forward State
    /// the PUBLISH asks for (draft-18 §10.19: its SUBSCRIBE_TRACKS's FORWARD).
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.downstream_publish",
        skip_all,
        fields(subscriber_session_id = %subscriber_session_id, track_namespace = %track_key.track_namespace, track_name = %track_key.track_name)
    )]
    pub(crate) async fn send(
        &self,
        subscriber_session_id: SessionId,
        track_key: &TrackKey,
        forward: bool,
    ) -> anyhow::Result<()> {
        let Some(upstream_track) = self.table.track_state(track_key) else {
            bail!("upstream track ended before PUBLISH was sent");
        };
        if self
            .table
            .is_publish_blocked(subscriber_session_id, track_key)
        {
            return Ok(());
        }
        let Some(_forwarding) = self
            .table
            .begin_forwarded_publish(track_key, subscriber_session_id)
        else {
            return Ok(());
        };
        let largest_location =
            existing_track_largest(&self.cache_store, track_key, &upstream_track);
        let content_exists = largest_location.map_or(ContentExists::False, |location| {
            ContentExists::True { location }
        });

        let published = self
            .forwarder
            .publish(
                subscriber_session_id,
                track_key.track_namespace.clone(),
                track_key.track_name.clone(),
                content_exists,
                upstream_track.track_properties(),
                forward,
            )
            .await;
        let downstream_subscription = match published {
            Ok(downstream_subscription) => downstream_subscription,
            Err(error)
                if matches!(
                    error.downcast_ref::<moqt::RequestFailure>(),
                    Some(moqt::RequestFailure::PublishBlocked)
                ) =>
            {
                tracing::info!(
                    "the subscriber had no stream for the PUBLISH; not sending it again"
                );
                self.table.block_publish(subscriber_session_id, track_key);
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let request_id = downstream_subscription.request_id();
        let Some(forward) = downstream_subscription.publish_ok_forward() else {
            bail!("PUBLISH answered without a publisher-initiated subscription");
        };
        let subscriber_peer = super::session_peer(subscriber_session_id, &self.forwarder).await;

        let filter_type = downstream_subscription.filter_type();
        let Some(runner_signals) = self.table.register_downstream_subscription(
            subscriber_session_id,
            request_id,
            subscriber_peer,
            DownstreamSubscription {
                track_key: track_key.clone(),
                filter_type,
                joining_location: largest_location,
            },
            DeliveryControl::new(forward, &filter_type, largest_location),
        ) else {
            let track_ended = PublishDoneReason {
                status_code: PublishDoneCode::TrackEnded,
                error_reason: "track ended before PUBLISH_OK".to_string(),
            };
            return self
                .forwarder
                .publish_done(subscriber_session_id, &downstream_subscription, track_ended)
                .await;
        };

        let (ready_sender, _ready_receiver) = oneshot::channel();
        let (publish_ok_sender, publish_ok_receiver) = oneshot::channel();
        let _ = publish_ok_sender.send(());
        if !start_egress(
            &self.forwarder,
            &self.cache_store,
            EgressRequest {
                subscriber_session_id,
                request_id,
                track_key: track_key.clone(),
                downstream_subscription,
                ready_sender,
                runner_signals,
                subscribe_ok_receiver: publish_ok_receiver,
                largest_location,
            },
        )
        .await
        {
            bail!("subscriber session left before its egress started");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moqt::namespace::NamespaceInterest;

    use crate::modules::{
        directory::track::{NamespaceSubscription, ProducerOrigin},
        domain::session_peer::SessionPeer,
        test_support::{
            directory_fixtures::{
                DownstreamPublishContext, NAMESPACE_SUBSCRIBER_SESSION, PUBLISHER_SESSION,
                downstream_publish_context, producer_with, table_with_upstream, track_key,
                upstream_carrying,
            },
            mock_session::{PUBLISH_REQUEST_ID, SentPublish, wait_until},
            relay_harness::fixtures::{
                cached_object::insert_closed_group, location,
                subscription::unknown_track_properties,
            },
        },
    };
    use moqt::TrackProperties;

    fn published_table() -> Arc<InMemoryLocalPubSubDirectory> {
        Arc::new(table_with_upstream(ProducerOrigin::Publish).0)
    }

    async fn context_accepting_publish() -> DownstreamPublishContext {
        downstream_publish_context(published_table(), || Ok(true)).await
    }

    async fn send_publish(ctx: &DownstreamPublishContext) -> anyhow::Result<()> {
        ctx.downstream_publish()
            .send(NAMESPACE_SUBSCRIBER_SESSION, &track_key(), true)
            .await
    }

    /// The egress runner holds the track cache while it runs.
    fn egress_runs(ctx: &DownstreamPublishContext, idle_cache_references: usize) -> bool {
        Arc::strong_count(&ctx.cache_store.get_or_create(&track_key())) > idle_cache_references + 1
    }

    #[tokio::test]
    async fn forwarded_publish_carries_the_upstream_track_properties() {
        // Arrange
        let mut track_properties = unknown_track_properties(1);
        track_properties.set_max_cache_duration(Some(2_000));
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_upstream_subscription(
            track_key(),
            PUBLISHER_SESSION,
            producer_with(upstream_carrying(track_properties.clone())),
        );
        let ctx = downstream_publish_context(Arc::new(table), || Ok(true)).await;

        // Act
        send_publish(&ctx).await.unwrap();

        // Assert
        let publishes = ctx.subscriber.publishes();
        assert_eq!(publishes.len(), 1);
        assert_eq!(publishes[0].track_properties, track_properties);
    }

    #[tokio::test]
    async fn publish_ok_registers_the_downstream_subscription_and_starts_its_egress() {
        // Arrange
        let ctx = context_accepting_publish().await;
        let idle_cache_references = Arc::strong_count(&ctx.cache_store.get_or_create(&track_key()));

        // Act
        send_publish(&ctx).await.unwrap();

        // Assert
        let registered = ctx
            .table
            .get_downstream_subscription(NAMESPACE_SUBSCRIBER_SESSION, PUBLISH_REQUEST_ID)
            .expect("PUBLISH_OK should register a downstream subscription");
        assert_eq!(registered.track_key, track_key());
        wait_until(|| egress_runs(&ctx, idle_cache_references))
            .await
            .expect("the forwarded PUBLISH should start an egress runner");
    }

    #[tokio::test]
    async fn publish_advertises_the_largest_location_egress_starts_after() {
        // Arrange
        let ctx = context_accepting_publish().await;
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
                track_properties: TrackProperties::default(),
                forward: true,
            }]
        );
        assert_eq!(
            ctx.table
                .get_downstream_subscription(NAMESPACE_SUBSCRIBER_SESSION, PUBLISH_REQUEST_ID)
                .map(|registered| registered.joining_location),
            Some(Some(location(4, 1)))
        );
    }

    #[tokio::test]
    async fn egress_starts_with_the_forward_state_of_the_publish_ok() {
        // Arrange
        let ctx = downstream_publish_context(published_table(), || Ok(false)).await;

        // Act
        send_publish(&ctx).await.unwrap();

        // Assert
        let control = ctx
            .table
            .downstream_delivery_control(NAMESPACE_SUBSCRIBER_SESSION, PUBLISH_REQUEST_ID)
            .expect("PUBLISH_OK should register a downstream subscription");
        assert!(!control.forward);
    }

    #[tokio::test]
    async fn publisher_session_cleanup_ends_the_forwarded_publish_with_publish_done() {
        // Arrange
        let ctx = context_accepting_publish().await;
        send_publish(&ctx).await.unwrap();

        // Act
        ctx.table.remove_session(PUBLISHER_SESSION);

        // Assert: the runner, stopped with a reason after its PUBLISH_OK, sends PUBLISH_DONE
        wait_until(|| {
            ctx.subscriber.publish_dones()
                == vec![(PUBLISH_REQUEST_ID, PublishDoneCode::TrackEnded)]
        })
        .await
        .expect("the runner should send PUBLISH_DONE TRACK_ENDED");
    }

    #[tokio::test]
    async fn publish_ok_after_the_upstream_ended_is_answered_with_publish_done() {
        // Arrange
        let table = published_table();
        let ended_table = table.clone();
        let ctx = downstream_publish_context(table, move || {
            ended_table.remove_session(PUBLISHER_SESSION);
            Ok(true)
        })
        .await;

        // Act
        send_publish(&ctx).await.unwrap();

        // Assert
        assert_eq!(
            ctx.subscriber.publish_dones(),
            vec![(PUBLISH_REQUEST_ID, PublishDoneCode::TrackEnded)]
        );
        assert!(ctx.table.downstream_subscription_states().is_empty());
    }

    #[tokio::test]
    async fn a_subscriber_already_receiving_the_track_gets_no_second_publish() {
        // Arrange
        let ctx = context_accepting_publish().await;
        send_publish(&ctx).await.unwrap();

        // Act
        send_publish(&ctx).await.unwrap();

        // Assert
        assert_eq!(ctx.subscriber.publishes().len(), 1);
        assert_eq!(ctx.table.downstream_subscription_states().len(), 1);
    }

    #[tokio::test]
    async fn publish_error_registers_nothing() {
        // Arrange
        let ctx =
            downstream_publish_context(published_table(), || anyhow::bail!("PUBLISH_ERROR")).await;

        // Act
        let result = send_publish(&ctx).await;

        // Assert
        assert!(result.is_err());
        assert!(ctx.table.downstream_subscription_states().is_empty());
    }

    fn blocked() -> anyhow::Result<bool> {
        Err(anyhow::Error::new(moqt::RequestFailure::PublishBlocked))
    }

    #[tokio::test]
    async fn a_track_whose_publish_was_blocked_is_never_published_to_the_subscriber_again() {
        // Arrange
        let ctx = downstream_publish_context(published_table(), blocked).await;
        let _ = send_publish(&ctx).await;

        // Act
        let _ = send_publish(&ctx).await;

        // Assert: draft-18 §6.1 MUST NOT send a PUBLISH for the track after PUBLISH_BLOCKED
        assert_eq!(ctx.subscriber.publishes().len(), 1);
        assert!(ctx.table.downstream_subscription_states().is_empty());
    }

    #[tokio::test]
    async fn a_new_tracks_subscription_gets_the_publish_an_ended_one_was_blocked_from() {
        // Arrange
        let answered = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ctx = downstream_publish_context(published_table(), {
            let answered = answered.clone();
            move || {
                if answered.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    Ok(true)
                } else {
                    blocked()
                }
            }
        })
        .await;
        let tracks = NamespaceSubscription {
            interest: NamespaceInterest::Tracks,
            peer: SessionPeer::Client,
            forward: true,
        };
        ctx.table.register_subscribe_namespace(
            NAMESPACE_SUBSCRIBER_SESSION,
            "ns".to_string(),
            tracks,
        );
        let _ = send_publish(&ctx).await;
        ctx.table.unregister_subscribe_namespace(
            NAMESPACE_SUBSCRIBER_SESSION,
            "ns",
            NamespaceInterest::Tracks,
        );
        ctx.table.register_subscribe_namespace(
            NAMESPACE_SUBSCRIBER_SESSION,
            "ns".to_string(),
            tracks,
        );

        // Act
        send_publish(&ctx).await.unwrap();

        // Assert: draft-18 §6.1, PUBLISH_BLOCKED bars the track from the subscription it went on only
        assert_eq!(ctx.subscriber.publishes().len(), 2);
        assert!(
            ctx.table
                .get_downstream_subscription(NAMESPACE_SUBSCRIBER_SESSION, PUBLISH_REQUEST_ID)
                .is_some()
        );
    }
}
