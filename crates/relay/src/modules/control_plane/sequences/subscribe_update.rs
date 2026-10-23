use moqt::object::Location;
use tracing::Span;

use super::subscribe::{cached_largest, existing_track_largest};
use crate::modules::{
    data_plane::cache::store::TrackCacheStore,
    domain::{
        delivery_control::{DeliveryControl, UpdateOutcome},
        pub_sub_directory::InMemoryLocalPubSubDirectory,
        session_id::SessionId,
    },
};

pub(crate) struct SubscribeUpdate;

impl SubscribeUpdate {
    /// Applies the Forward State, Start Location and End Group; Subscriber
    /// Priority keeps the value of the SUBSCRIBE. An update that widens the
    /// range is not treated as the PROTOCOL_VIOLATION draft-14 §9.10 allows:
    /// its widening part is ignored. `answer` accepts the update with the
    /// track's Largest Object (draft-18 §10.2.11) and returns whether the
    /// subscriber was given it, as `SubscribeUpdateHandler::ok` does; only
    /// then does an update that turns Forward on make it the subscription's
    /// Joining Location (draft-18 §5.1).
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe_update",
        skip_all,
        parent = session_span,
        fields(session_id = %session_id, subscription_request_id, update = ?update)
    )]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &InMemoryLocalPubSubDirectory,
        cache_store: &TrackCacheStore,
        subscription_request_id: u64,
        update: DeliveryControl,
        answer: impl FnOnce(Option<Location>) -> bool,
    ) {
        let Some((subscription, _)) =
            table.get_downstream_subscription(session_id, subscription_request_id)
        else {
            tracing::warn!("SUBSCRIBE_UPDATE for no active downstream subscription");
            answer(None);
            return;
        };
        let track_key = &subscription.track_key;
        let largest = match table.get_upstream_track(track_key) {
            Some(upstream_track) => existing_track_largest(cache_store, track_key, &upstream_track),
            None => cached_largest(cache_store, track_key),
        };
        let largest_told = answer(largest);
        match table.update_downstream_delivery(
            session_id,
            subscription_request_id,
            update,
            largest_told.then_some(largest),
        ) {
            Some(UpdateOutcome::Applied) => {
                tracing::info!("downstream subscription updated");
            }
            Some(UpdateOutcome::WideningIgnored) => {
                tracing::warn!(
                    "SUBSCRIBE_UPDATE widens the subscription; only its narrowing part is applied"
                );
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::test_support::directory_fixtures::downstream_subscription;
    use crate::modules::{
        domain::{
            pub_sub_directory::{DownstreamRunnerSignals, entry::UpstreamSubscriptionOrigin},
            session_peer::SessionPeer,
        },
        test_support::{
            directory_fixtures::{delivery_control, table_with_upstream},
            relay_harness::fixtures::{cached_object::insert_closed_group, location},
        },
    };

    fn table_with_downstream() -> (InMemoryLocalPubSubDirectory, DownstreamRunnerSignals) {
        let (table, upstream_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        let signals = table
            .register_downstream_subscription(
                2,
                100,
                SessionPeer::Client,
                downstream_subscription(upstream_key, None),
                delivery_control(true),
            )
            .unwrap();
        (table, signals)
    }

    #[test]
    fn forward_off_reaches_the_egress_runner_of_the_subscription() {
        // Arrange
        let (table, signals) = table_with_downstream();

        // Act
        SubscribeUpdate.handle(
            2,
            &Span::none(),
            &table,
            &TrackCacheStore::new(),
            100,
            delivery_control(false),
            |_| true,
        );

        // Assert
        assert!(!signals.delivery_control_receiver.borrow().forward);
    }

    #[test]
    fn end_group_reaches_the_egress_runner_of_the_subscription() {
        // Arrange
        let (table, signals) = table_with_downstream();
        let update = DeliveryControl::from_subscribe_update(true, location(0, 0), 4);

        // Act
        SubscribeUpdate.handle(
            2,
            &Span::none(),
            &table,
            &TrackCacheStore::new(),
            100,
            update,
            |_| true,
        );

        // Assert
        assert_eq!(
            signals.delivery_control_receiver.borrow().end_group,
            Some(4)
        );
    }

    #[test]
    fn widening_end_group_is_ignored() {
        // Arrange
        let (table, signals) = table_with_downstream();
        let narrowing = DeliveryControl::from_subscribe_update(true, location(0, 0), 4);
        SubscribeUpdate.handle(
            2,
            &Span::none(),
            &table,
            &TrackCacheStore::new(),
            100,
            narrowing,
            |_| true,
        );
        let open_ended = DeliveryControl::from_subscribe_update(true, location(0, 0), 0);

        // Act
        SubscribeUpdate.handle(
            2,
            &Span::none(),
            &table,
            &TrackCacheStore::new(),
            100,
            open_ended,
            |_| true,
        );

        // Assert
        assert_eq!(
            signals.delivery_control_receiver.borrow().end_group,
            Some(4)
        );
    }

    #[test]
    fn update_from_another_session_leaves_the_subscription_forwarding() {
        // Arrange
        let (table, signals) = table_with_downstream();

        // Act
        SubscribeUpdate.handle(
            3,
            &Span::none(),
            &table,
            &TrackCacheStore::new(),
            100,
            delivery_control(false),
            |_| true,
        );

        // Assert
        assert!(signals.delivery_control_receiver.borrow().forward);
    }

    /// A subscription accepted at `joining_location` on a track whose cache
    /// holds group 3 with objects 0 and 1.
    fn subscription_on_a_track_at_3_1(
        forward: bool,
        joining_location: Option<Location>,
    ) -> (InMemoryLocalPubSubDirectory, TrackCacheStore) {
        let (table, upstream_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        let cache_store = TrackCacheStore::new();
        insert_closed_group(&cache_store.get_or_create(&upstream_key), 3, &[0, 1]);
        table
            .register_downstream_subscription(
                2,
                100,
                SessionPeer::Client,
                downstream_subscription(upstream_key, joining_location),
                delivery_control(forward),
            )
            .unwrap();
        (table, cache_store)
    }

    fn joining_location(table: &InMemoryLocalPubSubDirectory) -> Option<Location> {
        let (subscription, _) = table.get_downstream_subscription(2, 100).unwrap();
        subscription.joining_location
    }

    /// Updates the subscription to Forward on; `tells` is what the answer
    /// returns. Returns the Largest Object the update was answered with.
    fn update_to_forward_on(
        table: &InMemoryLocalPubSubDirectory,
        cache_store: &TrackCacheStore,
        tells: bool,
    ) -> Option<Option<Location>> {
        let mut answered = None;
        SubscribeUpdate.handle(
            2,
            &Span::none(),
            table,
            cache_store,
            100,
            delivery_control(true),
            |largest| {
                answered = Some(largest);
                tells
            },
        );
        answered
    }

    #[test]
    fn forward_turned_on_moves_the_joining_location_to_the_largest_object_told() {
        // Arrange
        let (table, cache_store) = subscription_on_a_track_at_3_1(false, Some(location(1, 0)));

        // Act
        let answered = update_to_forward_on(&table, &cache_store, true);

        // Assert: draft-18 §10.2.11 LARGEST_OBJECT, §5.1 Joining Location
        assert_eq!(answered, Some(Some(location(3, 1))));
        assert_eq!(joining_location(&table), Some(location(3, 1)));
    }

    #[test]
    fn forward_turned_on_without_telling_the_largest_object_keeps_the_joining_location() {
        // Arrange
        let (table, cache_store) = subscription_on_a_track_at_3_1(false, Some(location(1, 0)));

        // Act
        let answered = update_to_forward_on(&table, &cache_store, false);

        // Assert: draft-14 §9.16.2 joins at the SUBSCRIBE_OK's Largest Location
        assert_eq!(answered, Some(Some(location(3, 1))));
        assert_eq!(joining_location(&table), Some(location(1, 0)));
    }

    #[test]
    fn an_update_keeping_forward_on_reports_the_largest_object_but_keeps_the_joining_location() {
        // Arrange
        let (table, cache_store) = subscription_on_a_track_at_3_1(true, Some(location(1, 0)));

        // Act
        let answered = update_to_forward_on(&table, &cache_store, true);

        // Assert
        assert_eq!(answered, Some(Some(location(3, 1))));
        assert_eq!(joining_location(&table), Some(location(1, 0)));
    }
}
