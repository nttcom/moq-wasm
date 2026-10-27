use moqt::subscribe::UpdateAnswer;
use tokio::sync::mpsc;
use tracing::Span;

use super::subscribe::{cached_largest, existing_track_largest};
use crate::modules::{
    data_plane::cache::store::TrackCacheStore,
    domain::{
        pub_sub_directory::{InMemoryLocalPubSubDirectory, entry::PublishDoneReason},
        session_id::SessionId,
    },
    session::{handler::subscribe_update::SubscribeUpdateHandler, session_event::SessionEvent},
};

pub(crate) struct SubscribeUpdate;

impl SubscribeUpdate {
    /// Applies the Forward State and the range of the update's filter;
    /// Subscriber Priority keeps the value of the SUBSCRIBE. An update that
    /// widens the range is applied only if the session's draft allows it
    /// (`may_widen`); otherwise the session is closed with
    /// PROTOCOL_VIOLATION (draft-14 §9.10). The update is accepted with the
    /// track's Largest Object (draft-18 §10.2.11); only when `ok` reports
    /// that the subscriber was given it does an update that turns Forward on
    /// make it the subscription's Joining Location (draft-18 §5.1).
    ///
    /// Returns the PUBLISH_DONE to end the subscription with when the update
    /// failed: `ok` reports an update answered REQUEST_ERROR for being
    /// answered too late, and draft-18 §10.9.1 then ends the subscription
    /// with UPDATE_FAILED.
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe_update",
        skip_all,
        parent = session_span,
        fields(session_id = %session_id, subscription_request_id)
    )]
    pub(crate) fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &InMemoryLocalPubSubDirectory,
        cache_store: &TrackCacheStore,
        session_event_sender: &mpsc::UnboundedSender<SessionEvent>,
        handler: &dyn SubscribeUpdateHandler,
    ) -> Option<PublishDoneReason> {
        let subscription_request_id = handler.subscription_request_id();
        Span::current().record("subscription_request_id", subscription_request_id);
        let Some((subscription, current)) =
            table.get_downstream_subscription(session_id, subscription_request_id)
        else {
            tracing::warn!("SUBSCRIBE_UPDATE for no active downstream subscription");
            handler.ok(None);
            return None;
        };
        let track_key = &subscription.track_key;
        let largest = match table.get_upstream_track(track_key) {
            Some(upstream_track) => existing_track_largest(cache_store, track_key, &upstream_track),
            None => cached_largest(cache_store, track_key),
        };
        let requested = current.requested(handler.forward(), handler.filter(), largest);
        if current.widens_to(&requested) && !handler.may_widen() {
            tracing::warn!(
                ?current,
                ?requested,
                "SUBSCRIBE_UPDATE widens the subscription"
            );
            let _ = session_event_sender.send(SessionEvent::protocol_violation_detected(
                session_id,
                "the update widens the subscription".to_string(),
            ));
            return None;
        }
        let UpdateAnswer::Accepted { largest_told } = handler.ok(largest) else {
            tracing::warn!("SUBSCRIBE_UPDATE answered too late; ending the subscription");
            return Some(PublishDoneReason::update_failed());
        };
        if table.update_downstream_delivery(
            session_id,
            subscription_request_id,
            requested,
            largest_told.then_some(largest),
        ) {
            tracing::info!(?requested, "downstream subscription updated");
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use moqt::{object::Location, subscribe::FilterType};

    use super::*;
    use crate::modules::test_support::directory_fixtures::downstream_subscription;
    use crate::modules::{
        domain::{
            delivery_control::DeliveryControl,
            pub_sub_directory::{DownstreamRunnerSignals, entry::UpstreamSubscriptionOrigin},
            session_peer::SessionPeer,
        },
        session::session_event::EventKind,
        test_support::{
            directory_fixtures::{delivery_control, table_with_upstream},
            mock_session::MockSubscribeUpdateHandler,
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
        session_event_receiver: mpsc::UnboundedReceiver<SessionEvent>,
    }

    impl Updated {
        fn control(&self) -> DeliveryControl {
            *self.signals.delivery_control_receiver.borrow()
        }

        fn closed_the_session(&mut self) -> bool {
            matches!(
                self.session_event_receiver.try_recv(),
                Ok(SessionEvent {
                    session_id: SUBSCRIBER_SESSION,
                    kind: EventKind::ProtocolViolationDetected { .. },
                })
            )
        }
    }

    /// A subscription with `control` on a track whose cache holds group 3
    /// with objects 0 and 1.
    fn subscription_with(control: DeliveryControl) -> Updated {
        let (table, upstream_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
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
        let (_, session_event_receiver) = mpsc::unbounded_channel();
        Updated {
            table,
            cache_store,
            signals,
            session_event_receiver,
        }
    }

    fn update(
        updated: &mut Updated,
        session_id: SessionId,
        handler: &MockSubscribeUpdateHandler,
    ) -> Option<PublishDoneReason> {
        let (session_event_sender, session_event_receiver) = mpsc::unbounded_channel();
        updated.session_event_receiver = session_event_receiver;
        SubscribeUpdate.handle(
            session_id,
            &Span::none(),
            &updated.table,
            &updated.cache_store,
            &session_event_sender,
            handler,
        )
    }

    fn joining_location(updated: &Updated) -> Option<Location> {
        let (subscription, _) = updated
            .table
            .get_downstream_subscription(SUBSCRIBER_SESSION, SUBSCRIPTION)
            .unwrap();
        subscription.joining_location
    }

    fn starting_at(location: Location) -> DeliveryControl {
        DeliveryControl::new(true, &FilterType::AbsoluteStart { location }, None)
    }

    #[test]
    fn forward_off_reaches_the_egress_runner_of_the_subscription() {
        // Arrange
        let mut updated = subscription_with(delivery_control(true));
        let handler = MockSubscribeUpdateHandler::draft14(SUBSCRIPTION, false, Some(FROM_START));

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert
        assert!(!updated.control().forward);
    }

    #[test]
    fn end_group_reaches_the_egress_runner_of_the_subscription() {
        // Arrange
        let mut updated = subscription_with(delivery_control(true));
        let filter = FilterType::AbsoluteRange {
            location: location(0, 0),
            end_group: 3,
        };
        let handler = MockSubscribeUpdateHandler::draft14(SUBSCRIPTION, true, Some(filter));

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert
        assert_eq!(updated.control().end_group, Some(4));
        assert!(!updated.closed_the_session());
    }

    #[test]
    fn a_draft14_update_moving_the_start_location_back_closes_the_session() {
        // Arrange
        let mut updated = subscription_with(starting_at(location(5, 0)));
        let earlier_start = FilterType::AbsoluteStart {
            location: location(4, 0),
        };
        let handler = MockSubscribeUpdateHandler::draft14(SUBSCRIPTION, true, Some(earlier_start));

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert: draft-14 §9.10
        assert!(updated.closed_the_session());
        assert_eq!(updated.control().start, location(5, 0));
    }

    #[test]
    fn a_draft14_update_moving_the_end_group_out_closes_the_session() {
        // Arrange
        let range = FilterType::AbsoluteRange {
            location: location(0, 0),
            end_group: 4,
        };
        let mut updated = subscription_with(DeliveryControl::new(true, &range, None));
        let handler = MockSubscribeUpdateHandler::draft14(SUBSCRIPTION, true, Some(FROM_START));

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert: draft-14 §9.10
        assert!(updated.closed_the_session());
        assert_eq!(updated.control().end_group, Some(5));
    }

    #[test]
    fn a_draft18_update_moving_the_start_back_and_the_end_out_takes_the_new_range() {
        // Arrange
        let range = FilterType::AbsoluteRange {
            location: location(5, 0),
            end_group: 6,
        };
        let mut updated = subscription_with(DeliveryControl::new(true, &range, None));
        let wider = FilterType::AbsoluteStart {
            location: location(2, 0),
        };
        let handler = MockSubscribeUpdateHandler::draft18(SUBSCRIPTION, true, Some(wider));

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert: draft-18 §10.9.1
        assert!(!updated.closed_the_session());
        let control = updated.control();
        assert_eq!((control.start, control.end_group), (location(2, 0), None));
        assert_eq!(control.largest_at_update, Some(location(3, 1)));
    }

    #[test]
    fn the_answer_to_a_widening_draft18_update_carries_the_largest_object() {
        // Arrange
        let mut updated = subscription_with(starting_at(location(5, 0)));
        let earlier_start = FilterType::AbsoluteStart {
            location: location(2, 0),
        };
        let handler = MockSubscribeUpdateHandler::draft18(SUBSCRIPTION, true, Some(earlier_start));

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert: draft-18 §10.9.1, the subscriber FETCHes the gap up to it
        assert_eq!(*handler.answers.lock().unwrap(), vec![Some(location(3, 1))]);
    }

    #[test]
    fn an_update_without_a_filter_keeps_the_range() {
        // Arrange
        let mut updated = subscription_with(starting_at(location(5, 0)));
        let handler = MockSubscribeUpdateHandler::draft18(SUBSCRIPTION, false, None);

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert: draft-18 §10.2.9
        let control = updated.control();
        assert_eq!((control.forward, control.start), (false, location(5, 0)));
    }

    #[test]
    fn an_update_answered_after_its_timeout_ends_the_subscription_with_update_failed() {
        // Arrange
        let mut updated = subscription_with(delivery_control(true));
        let handler = MockSubscribeUpdateHandler::draft18_timed_out(SUBSCRIPTION, false, None);

        // Act
        let end = update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert: draft-18 §10.9.1, and the failed update is not applied
        assert_eq!(end, Some(PublishDoneReason::update_failed()));
        assert!(updated.control().forward);
    }

    #[test]
    fn update_from_another_session_leaves_the_subscription_forwarding() {
        // Arrange
        let mut updated = subscription_with(delivery_control(true));
        let handler = MockSubscribeUpdateHandler::draft14(SUBSCRIPTION, false, Some(FROM_START));

        // Act
        update(&mut updated, 3, &handler);

        // Assert
        assert!(updated.control().forward);
    }

    #[test]
    fn forward_turned_on_moves_the_joining_location_to_the_largest_object_told() {
        // Arrange
        let mut updated = subscription_with(delivery_control(false));
        let handler = MockSubscribeUpdateHandler::draft18(SUBSCRIPTION, true, None);

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert: draft-18 §10.2.11 LARGEST_OBJECT, §5.1 Joining Location
        assert_eq!(*handler.answers.lock().unwrap(), vec![Some(location(3, 1))]);
        assert_eq!(joining_location(&updated), Some(location(3, 1)));
    }

    #[test]
    fn forward_turned_on_without_telling_the_largest_object_keeps_the_joining_location() {
        // Arrange
        let mut updated = subscription_with(delivery_control(false));
        let handler = MockSubscribeUpdateHandler::draft14(SUBSCRIPTION, true, Some(FROM_START));

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert: draft-14 §9.16.2 joins at the SUBSCRIBE_OK's Largest Location
        assert_eq!(*handler.answers.lock().unwrap(), vec![Some(location(3, 1))]);
        assert_eq!(joining_location(&updated), Some(location(1, 0)));
    }

    #[test]
    fn an_update_keeping_forward_on_reports_the_largest_object_but_keeps_the_joining_location() {
        // Arrange
        let mut updated = subscription_with(delivery_control(true));
        let handler = MockSubscribeUpdateHandler::draft18(SUBSCRIPTION, true, None);

        // Act
        update(&mut updated, SUBSCRIBER_SESSION, &handler);

        // Assert
        assert_eq!(*handler.answers.lock().unwrap(), vec![Some(location(3, 1))]);
        assert_eq!(joining_location(&updated), Some(location(1, 0)));
    }
}
