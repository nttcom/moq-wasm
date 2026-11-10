use moqt::subscribe::UpdateAnswer;

use super::subscribe::{cached_largest, existing_track_largest};
use crate::modules::{
    data_plane::cache::store::TrackCacheStore,
    directory::InMemoryLocalPubSubDirectory,
    domain::{publish_done_reason::PublishDoneReason, session_id::SessionId},
    session::handler::subscribe_update::SubscribeUpdateHandler,
};

pub(crate) struct SubscribeUpdate;

/// What the session worker does after an update.
#[derive(Debug, PartialEq)]
pub(crate) enum UpdateVerdict {
    Done,
    /// End the subscription with this PUBLISH_DONE.
    EndSubscription(PublishDoneReason),
    /// Close the session with PROTOCOL_VIOLATION for this reason.
    CloseSession(&'static str),
}

impl SubscribeUpdate {
    /// Applies the Subscriber Priority, the Forward State and the range of
    /// the update's filter. An update that widens the range is applied only
    /// if the session's draft allows it (`may_widen`); otherwise the session
    /// is closed with PROTOCOL_VIOLATION (draft-14 §9.10). The update is
    /// accepted with the track's Largest Object (draft-18 §10.2.11); only
    /// when `ok` reports that the subscriber was given it does an update
    /// that turns Forward on make it the subscription's Joining Location
    /// (draft-18 §5.1).
    ///
    /// The subscription ends with UPDATE_FAILED when `ok` reports the update
    /// failed: moqt answered it REQUEST_ERROR for coming too late, and
    /// draft-18 §10.9.1 then requires UPDATE_FAILED.
    pub(crate) fn handle(
        &self,
        session_id: SessionId,
        table: &InMemoryLocalPubSubDirectory,
        cache_store: &TrackCacheStore,
        handler: &dyn SubscribeUpdateHandler,
    ) -> UpdateVerdict {
        let subscription_request_id = handler.subscription_request_id();
        let (Some(subscription), Some(current)) = (
            table.get_downstream_subscription(session_id, subscription_request_id),
            table.downstream_delivery_control(session_id, subscription_request_id),
        ) else {
            tracing::warn!(
                session_id,
                subscription_request_id,
                "SUBSCRIBE_UPDATE for no active downstream subscription"
            );
            handler.ok(None);
            return UpdateVerdict::Done;
        };
        let track_key = &subscription.track_key;
        let largest = match table.track_state(track_key) {
            Some(track) => existing_track_largest(cache_store, track_key, &track),
            None => cached_largest(cache_store, track_key),
        };
        let requested = current.requested(
            handler.subscriber_priority(),
            handler.forward(),
            handler.filter(),
            largest,
        );
        if current.widens_to(&requested) && !handler.may_widen() {
            tracing::warn!(
                session_id,
                subscription_request_id,
                ?current,
                ?requested,
                "SUBSCRIBE_UPDATE widens the subscription"
            );
            return UpdateVerdict::CloseSession("the update widens the subscription");
        }
        let UpdateAnswer::Accepted { largest_told } = handler.ok(largest) else {
            tracing::warn!(
                session_id,
                subscription_request_id,
                "SUBSCRIBE_UPDATE answered too late; ending the subscription"
            );
            return UpdateVerdict::EndSubscription(PublishDoneReason::update_failed());
        };
        if table.update_downstream_delivery(
            session_id,
            subscription_request_id,
            requested,
            largest_told.then_some(largest),
        ) {
            tracing::info!(
                session_id,
                subscription_request_id,
                ?requested,
                "downstream subscription updated"
            );
        }
        UpdateVerdict::Done
    }
}

#[cfg(test)]
mod tests {
    use moqt::{object::Location, subscribe::FilterType};

    use super::*;
    use crate::modules::{
        directory::track::ProducerOrigin,
        domain::{
            delivery_control::DeliveryControl, runner_signals::DownstreamRunnerSignals,
            session_peer::SessionPeer,
        },
        test_support::{
            directory_fixtures::{delivery_control, downstream_subscription, table_with_upstream},
            mock_session::{MockSubscribeUpdateHandler, UpdateDraft},
            relay_harness::fixtures::{cached_object::insert_closed_group, location},
        },
    };

    const SUBSCRIBER_SESSION: SessionId = 2;
    const SUBSCRIPTION: u64 = 100;

    /// The subscription's current range repeated, as a draft-14 update does.
    const FROM_START: FilterType = FilterType::AbsoluteStart {
        location: Location {
            group_id: 0,
            object_id: 0,
        },
    };

    struct Updated {
        table: InMemoryLocalPubSubDirectory,
        cache_store: TrackCacheStore,
        signals: DownstreamRunnerSignals,
        verdict: Option<UpdateVerdict>,
    }

    impl Updated {
        fn control(&self) -> DeliveryControl {
            *self.signals.delivery_control_receiver.borrow()
        }

        fn closed_the_session(&self) -> bool {
            matches!(self.verdict, Some(UpdateVerdict::CloseSession(_)))
        }
    }

    /// A subscription with `control` on a track whose cache holds group 3
    /// with objects 0 and 1.
    fn subscription_with(control: DeliveryControl) -> Updated {
        let (table, upstream_key) = table_with_upstream(ProducerOrigin::Subscribe);
        let cache_store = TrackCacheStore::new();
        insert_closed_group(&cache_store.get_or_create(&upstream_key), 3, &[0, 1]);
        let signals = table
            .register_downstream_subscription(
                SUBSCRIBER_SESSION,
                SUBSCRIPTION,
                SessionPeer::Client,
                downstream_subscription(upstream_key, Some(location(1, 0))),
                control,
            )
            .unwrap();
        Updated {
            table,
            cache_store,
            signals,
            verdict: None,
        }
    }

    fn update(
        updated: &mut Updated,
        session_id: SessionId,
        handler: &MockSubscribeUpdateHandler,
    ) -> Option<PublishDoneReason> {
        let verdict =
            SubscribeUpdate.handle(session_id, &updated.table, &updated.cache_store, handler);
        let end = match &verdict {
            UpdateVerdict::EndSubscription(end) => Some(end.clone()),
            _ => None,
        };
        updated.verdict = Some(verdict);
        end
    }

    fn joining_location(updated: &Updated) -> Option<Location> {
        updated
            .table
            .get_downstream_subscription(SUBSCRIBER_SESSION, SUBSCRIPTION)
            .unwrap()
            .joining_location
    }

    fn starting_at(location: Location) -> DeliveryControl {
        DeliveryControl::new(128, true, &FilterType::AbsoluteStart { location }, None)
    }

    #[tokio::test]
    async fn forward_off_reaches_the_egress_runner_of_the_subscription() {
        // Arrange
        let mut updated = subscription_with(delivery_control(true));
        let handler = MockSubscribeUpdateHandler::new(
            UpdateDraft::Draft14,
            SUBSCRIPTION,
            false,
            Some(FROM_START),
        );

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert
        assert!(!updated.control().forward);
    }

    #[tokio::test]
    async fn subscriber_priority_reaches_the_egress_runner_of_the_subscription() {
        // Arrange
        let mut updated = subscription_with(delivery_control(true));
        let handler = MockSubscribeUpdateHandler::new(
            UpdateDraft::Draft14,
            SUBSCRIPTION,
            true,
            Some(FROM_START),
        )
        .with_subscriber_priority(7);

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert
        assert_eq!(updated.control().subscriber_priority, 7);
        assert!(!updated.closed_the_session());
    }

    #[tokio::test]
    async fn end_group_reaches_the_egress_runner_of_the_subscription() {
        // Arrange
        let mut updated = subscription_with(delivery_control(true));
        let filter = FilterType::AbsoluteRange {
            location: location(0, 0),
            end_group: 3,
        };
        let handler =
            MockSubscribeUpdateHandler::new(UpdateDraft::Draft14, SUBSCRIPTION, true, Some(filter));

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert
        assert_eq!(updated.control().end_group, Some(4));
        assert!(!updated.closed_the_session());
    }

    #[tokio::test]
    async fn a_draft14_update_moving_the_start_location_back_closes_the_session() {
        // Arrange
        let mut updated = subscription_with(starting_at(location(5, 0)));
        let earlier_start = FilterType::AbsoluteStart {
            location: location(4, 0),
        };
        let handler = MockSubscribeUpdateHandler::new(
            UpdateDraft::Draft14,
            SUBSCRIPTION,
            true,
            Some(earlier_start),
        );

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert: draft-14 §9.10
        assert!(updated.closed_the_session());
        assert_eq!(updated.control().start, location(5, 0));
    }

    #[tokio::test]
    async fn a_draft14_update_moving_the_end_group_out_closes_the_session() {
        // Arrange
        let range = FilterType::AbsoluteRange {
            location: location(0, 0),
            end_group: 4,
        };
        let mut updated = subscription_with(DeliveryControl::new(128, true, &range, None));
        let handler = MockSubscribeUpdateHandler::new(
            UpdateDraft::Draft14,
            SUBSCRIPTION,
            true,
            Some(FROM_START),
        );

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert: draft-14 §9.10
        assert!(updated.closed_the_session());
        assert_eq!(updated.control().end_group, Some(5));
    }

    #[tokio::test]
    async fn a_draft18_update_moving_the_start_back_and_the_end_out_takes_the_new_range() {
        // Arrange
        let range = FilterType::AbsoluteRange {
            location: location(5, 0),
            end_group: 6,
        };
        let mut updated = subscription_with(DeliveryControl::new(128, true, &range, None));
        let wider = FilterType::AbsoluteStart {
            location: location(2, 0),
        };
        let handler =
            MockSubscribeUpdateHandler::new(UpdateDraft::Draft18, SUBSCRIPTION, true, Some(wider));

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert: draft-18 §10.9.1
        assert!(!updated.closed_the_session());
        let control = updated.control();
        assert_eq!((control.start, control.end_group), (location(2, 0), None));
        assert_eq!(control.largest_at_update, Some(location(3, 1)));
    }

    #[tokio::test]
    async fn the_answer_to_a_widening_draft18_update_carries_the_largest_object() {
        // Arrange
        let mut updated = subscription_with(starting_at(location(5, 0)));
        let earlier_start = FilterType::AbsoluteStart {
            location: location(2, 0),
        };
        let handler = MockSubscribeUpdateHandler::new(
            UpdateDraft::Draft18,
            SUBSCRIPTION,
            true,
            Some(earlier_start),
        );

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert: draft-18 §10.9.1, the subscriber FETCHes the gap up to it
        assert_eq!(*handler.answers.lock().unwrap(), vec![Some(location(3, 1))]);
    }

    #[tokio::test]
    async fn an_update_without_a_filter_keeps_the_range() {
        // Arrange
        let mut updated = subscription_with(starting_at(location(5, 0)));
        let handler =
            MockSubscribeUpdateHandler::new(UpdateDraft::Draft18, SUBSCRIPTION, false, None);

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert: draft-18 §10.2.9
        let control = updated.control();
        assert_eq!((control.forward, control.start), (false, location(5, 0)));
    }

    #[tokio::test]
    async fn an_update_answered_after_its_timeout_ends_the_subscription_with_update_failed() {
        // Arrange
        let mut updated = subscription_with(delivery_control(true));
        let handler = MockSubscribeUpdateHandler::new(
            UpdateDraft::Draft18TimedOut,
            SUBSCRIPTION,
            false,
            None,
        );

        // Act
        let end = update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert: draft-18 §10.9.1, and the failed update is not applied
        assert_eq!(end, Some(PublishDoneReason::update_failed()));
        assert!(updated.control().forward);
    }

    #[tokio::test]
    async fn update_from_another_session_leaves_the_subscription_forwarding() {
        // Arrange
        let mut updated = subscription_with(delivery_control(true));
        let handler = MockSubscribeUpdateHandler::new(
            UpdateDraft::Draft14,
            SUBSCRIPTION,
            false,
            Some(FROM_START),
        );

        // Act
        update(&mut updated, 3, &handler);

        // Assert
        assert!(updated.control().forward);
    }

    #[tokio::test]
    async fn forward_turned_on_moves_the_joining_location_to_the_largest_object_told() {
        // Arrange
        let mut updated = subscription_with(delivery_control(false));
        let handler =
            MockSubscribeUpdateHandler::new(UpdateDraft::Draft18, SUBSCRIPTION, true, None);

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert: draft-18 §10.2.11 LARGEST_OBJECT, §5.1 Joining Location
        assert_eq!(*handler.answers.lock().unwrap(), vec![Some(location(3, 1))]);
        assert_eq!(joining_location(&updated), Some(location(3, 1)));
    }

    #[tokio::test]
    async fn forward_turned_on_without_telling_the_largest_object_keeps_the_joining_location() {
        // Arrange
        let mut updated = subscription_with(delivery_control(false));
        let handler = MockSubscribeUpdateHandler::new(
            UpdateDraft::Draft14,
            SUBSCRIPTION,
            true,
            Some(FROM_START),
        );

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert: draft-14 §9.16.2 joins at the SUBSCRIBE_OK's Largest Location
        assert_eq!(*handler.answers.lock().unwrap(), vec![Some(location(3, 1))]);
        assert_eq!(joining_location(&updated), Some(location(1, 0)));
    }

    #[tokio::test]
    async fn an_update_keeping_forward_on_reports_the_largest_object_but_keeps_the_joining_location()
     {
        // Arrange
        let mut updated = subscription_with(delivery_control(true));
        let handler =
            MockSubscribeUpdateHandler::new(UpdateDraft::Draft18, SUBSCRIPTION, true, None);

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert
        assert_eq!(*handler.answers.lock().unwrap(), vec![Some(location(3, 1))]);
        assert_eq!(joining_location(&updated), Some(location(1, 0)));
    }
}
