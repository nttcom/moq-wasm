use std::time::Duration;

use moqt::{object::Location, subscribe::FilterType};

/// The part of a downstream subscription a SUBSCRIBE_UPDATE / REQUEST_UPDATE
/// can change (draft-14 §9.10, draft-18 §10.9.1), shared by its registration
/// and its egress runner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DeliveryControl {
    pub(crate) subscriber_priority: u8,
    pub(crate) forward: bool,
    /// The subscription's Start Location (§9.7), before egress raises it
    /// past the Largest Object. `None` when no Location follows the one its
    /// filter starts after, so it delivers nothing; see `starts_before`.
    pub(crate) start: Option<Location>,
    /// The first group past the subscription; `None` is open-ended.
    pub(crate) end_group: Option<u64>,
    /// The track's Largest Object when the subscription was accepted or last
    /// updated. Delivery restarted by an update (a Start Location moved
    /// back, draft-18 §10.9.1, or Forward turned on) sends only the objects
    /// after it, as a new subscription would; FETCH serves the rest.
    pub(crate) largest_at_update: Option<Location>,
    /// Counts the updates that moved the Start Location back, so egress
    /// restarts delivery even when it sees only the latest state.
    pub(crate) restarts: u64,
    /// Counts the updates that turned Forward from 0 to 1. A stream opened
    /// before the latest one is not continued: Forward 0 sends no object
    /// (draft-14 §5.1, draft-18 §5.1), even when egress never saw it.
    pub(crate) resumes: u64,
    /// The subscriber's DELIVERY TIMEOUT (draft-14 §9.2.1.2): how long after
    /// the relay received an object it is still worth sending. `None` sends
    /// every object.
    pub(crate) delivery_timeout: Option<Duration>,
}

impl DeliveryControl {
    /// `largest` is the Largest Location the subscription was accepted with.
    /// An AbsoluteRange End Group is the last group included (§9.7).
    pub(crate) fn new(
        subscriber_priority: u8,
        forward: bool,
        filter_type: &FilterType,
        largest: Option<Location>,
    ) -> Self {
        let end_group = match filter_type {
            FilterType::AbsoluteRange { end_group, .. } => Some(end_group.saturating_add(1)),
            _ => None,
        };
        Self {
            subscriber_priority,
            forward,
            start: filter_type.start_location(largest),
            end_group,
            largest_at_update: largest,
            restarts: 0,
            resumes: 0,
            delivery_timeout: None,
        }
    }

    /// `millis` is the DELIVERY TIMEOUT value; 0 sets no timeout (draft-18 §8).
    pub(crate) fn with_delivery_timeout(self, millis: Option<u64>) -> Self {
        Self {
            delivery_timeout: millis
                .filter(|&millis| millis > 0)
                .map(Duration::from_millis),
            ..self
        }
    }

    /// `end_group` is the SUBSCRIBE_UPDATE field: the end group plus 1, with
    /// 0 meaning open-ended (§9.10).
    #[cfg(test)]
    pub(crate) fn from_subscribe_update(forward: bool, start: Location, end_group: u64) -> Self {
        Self {
            subscriber_priority: 128,
            forward,
            start: Some(start),
            end_group: (end_group != 0).then_some(end_group),
            largest_at_update: None,
            restarts: 0,
            resumes: 0,
            delivery_timeout: None,
        }
    }

    /// What an update asks for when the track's Largest Object is `largest`:
    /// its Subscriber Priority, its Forward State and the range of `filter`,
    /// resolved like a SUBSCRIBE's; `None` keeps the current range.
    pub(crate) fn requested(
        &self,
        subscriber_priority: u8,
        forward: bool,
        filter: Option<FilterType>,
        largest: Option<Location>,
    ) -> Self {
        let range = match filter {
            Some(filter) => Self::new(subscriber_priority, forward, &filter, largest),
            None => *self,
        };
        Self {
            subscriber_priority,
            forward,
            largest_at_update: largest,
            restarts: self.restarts,
            resumes: self.resumes,
            delivery_timeout: self.delivery_timeout,
            ..range
        }
    }

    /// Whether `requested` moves the Start Location back or the End Group out.
    pub(crate) fn widens_to(&self, requested: &Self) -> bool {
        let end_moves_out = match (self.end_group, requested.end_group) {
            (Some(current), Some(requested)) => requested > current,
            (Some(_), None) => true,
            (None, _) => false,
        };
        starts_before(requested.start, self.start) || end_moves_out
    }

    pub(crate) fn contains_group(&self, group_id: u64) -> bool {
        self.start.is_some_and(|start| group_id >= start.group_id) && !self.is_past_end(group_id)
    }

    pub(crate) fn is_past_end(&self, group_id: u64) -> bool {
        self.end_group.is_some_and(|end| group_id >= end)
    }

    /// Takes `requested`, which the session's draft has already allowed (see
    /// `widens_to`); counts a Start Location moved back in `restarts` and
    /// Forward 0→1 in `resumes`.
    pub(crate) fn apply_update(&mut self, requested: DeliveryControl) {
        let restarts = self.restarts + u64::from(starts_before(requested.start, self.start));
        let resumes = self.resumes + u64::from(!self.forward && requested.forward);
        *self = Self {
            restarts,
            resumes,
            ..requested
        };
    }

    /// Where a subgroup of `group_id` continues once an update lets it be
    /// delivered again, given that `object_id` is the next one it has not
    /// sent: past the Largest Object at that update. `None` when the whole
    /// group lies at or before it.
    pub(crate) fn resume_object_id(&self, group_id: u64, object_id: u64) -> Option<u64> {
        match self.largest_at_update {
            Some(largest) if group_id < largest.group_id => None,
            Some(largest) if group_id == largest.group_id => largest
                .object_id
                .checked_add(1)
                .map(|next| object_id.max(next)),
            _ => Some(object_id),
        }
    }
}

/// Whether Start Location `start` precedes `other`, with `None` (delivers
/// nothing) after every Location: the reverse of `Option`'s order.
pub(crate) fn starts_before(start: Option<Location>, other: Option<Location>) -> bool {
    match (start, other) {
        (Some(start), Some(other)) => start < other,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::modules::test_support::relay_harness::fixtures::location;

    fn id_near_bounds() -> impl Strategy<Value = u64> {
        prop_oneof![
            Just(0),
            Just(1),
            Just(u64::MAX - 1),
            Just(u64::MAX),
            0..8u64,
            any::<u64>(),
        ]
    }

    fn location_near_bounds() -> impl Strategy<Value = Location> {
        (id_near_bounds(), id_near_bounds())
            .prop_map(|(group_id, object_id)| location(group_id, object_id))
    }

    fn bounded(start: Location, end_group: u64) -> DeliveryControl {
        DeliveryControl::from_subscribe_update(true, start, end_group)
    }

    #[test]
    fn start_follows_the_filter_and_the_largest_location() {
        // Arrange
        let largest = Some(location(5, 3));

        // Act
        let control = DeliveryControl::new(128, true, &FilterType::NextGroupStart, largest);

        // Assert
        assert_eq!(control.start, Some(location(6, 0)));
        assert_eq!(control.end_group, None);
    }

    #[test]
    fn next_group_start_in_the_largest_group_contains_no_group() {
        // Act
        let control = DeliveryControl::new(
            128,
            true,
            &FilterType::NextGroupStart,
            Some(location(u64::MAX, 3)),
        );

        // Assert
        assert_eq!(control.start, None);
        assert!(!control.contains_group(u64::MAX));
    }

    #[test]
    fn a_start_that_delivers_nothing_follows_every_location() {
        // Arrange
        let empty =
            DeliveryControl::new(128, true, &FilterType::LargestObject, Some(Location::MAX));
        let last = FilterType::AbsoluteStart {
            location: Location::MAX,
        };

        // Act
        let requested = empty.requested(128, true, Some(last), Some(Location::MAX));

        // Assert: a start at the last Location moves back, which restarts delivery from past the Largest Object
        assert!(empty.widens_to(&requested));
        assert!(!requested.widens_to(&empty));
    }

    #[test]
    fn absolute_range_includes_its_end_group() {
        // Arrange
        let filter_type = FilterType::AbsoluteRange {
            location: location(2, 0),
            end_group: 4,
        };

        // Act
        let control = DeliveryControl::new(128, true, &filter_type, None);

        // Assert
        assert!(control.contains_group(4));
        assert!(!control.contains_group(5));
        assert!(!control.contains_group(1));
    }

    #[test]
    fn subscribe_update_end_group_zero_is_open_ended() {
        // Arrange / Act
        let control = DeliveryControl::from_subscribe_update(true, location(0, 0), 0);

        // Assert
        assert_eq!(control.end_group, None);
    }

    #[test]
    fn subscribe_update_end_group_excludes_its_value() {
        // Arrange / Act
        let control = DeliveryControl::from_subscribe_update(true, location(0, 0), 5);

        // Assert
        assert!(control.contains_group(4));
        assert!(!control.contains_group(5));
    }

    #[test]
    fn narrowing_update_moves_start_and_end() {
        // Arrange
        let mut control = DeliveryControl::new(128, true, &FilterType::LargestObject, None);
        let filter = FilterType::AbsoluteRange {
            location: location(3, 1),
            end_group: 7,
        };
        let requested = control.requested(128, true, Some(filter), None);

        // Act
        let widens = control.widens_to(&requested);
        control.apply_update(requested);

        // Assert
        assert!(!widens);
        assert_eq!(control, bounded(location(3, 1), 8));
    }

    #[test]
    fn moving_the_start_back_or_the_end_out_widens() {
        // Arrange
        let control = bounded(location(3, 0), 8);
        let earlier_start = FilterType::AbsoluteRange {
            location: location(2, 0),
            end_group: 6,
        };
        let open_end = FilterType::AbsoluteStart {
            location: location(3, 0),
        };

        // Act
        let start_back = control.requested(128, true, Some(earlier_start), None);
        let end_out = control.requested(128, true, Some(open_end), None);

        // Assert
        assert!(control.widens_to(&start_back));
        assert!(control.widens_to(&end_out));
    }

    #[test]
    fn moving_the_start_back_counts_a_restart() {
        // Arrange
        let mut control = bounded(location(3, 0), 8);
        let earlier_start = FilterType::AbsoluteStart {
            location: location(2, 0),
        };
        let later_start = FilterType::AbsoluteStart {
            location: location(4, 0),
        };

        // Act
        control.apply_update(control.requested(128, true, Some(later_start), None));
        let after_narrowing = control.restarts;
        control.apply_update(control.requested(128, true, Some(earlier_start), None));

        // Assert
        assert_eq!((after_narrowing, control.restarts), (0, 1));
    }

    #[test]
    fn a_subgroup_continues_past_the_largest_object_at_the_update() {
        // Arrange
        let mut control = DeliveryControl::new(128, true, &FilterType::LargestObject, None);

        // Act
        control.apply_update(control.requested(128, true, None, Some(location(4, 2))));

        // Assert
        assert_eq!(control.resume_object_id(3, 7), None);
        assert_eq!(control.resume_object_id(4, 1), Some(3));
        assert_eq!(control.resume_object_id(4, 5), Some(5));
        assert_eq!(control.resume_object_id(5, 0), Some(0));
    }

    #[test]
    fn nothing_of_a_group_follows_a_largest_object_at_the_largest_object_id() {
        // Arrange
        let mut control = DeliveryControl::new(128, true, &FilterType::LargestObject, None);
        let mut last = control;

        // Act
        control.apply_update(control.requested(128, true, None, Some(location(4, u64::MAX))));
        last.apply_update(last.requested(128, true, None, Some(Location::MAX)));

        // Assert
        assert_eq!(control.resume_object_id(4, 7), None);
        assert_eq!(control.resume_object_id(5, 0), Some(0));
        assert_eq!(last.resume_object_id(u64::MAX, 7), None);
    }

    proptest! {
        #[test]
        fn a_subgroup_resumes_at_the_first_object_after_the_largest_object(
            (largest, group_id) in location_near_bounds().prop_flat_map(|largest| {
                let near = prop_oneof![
                    Just(largest.group_id),
                    Just(largest.group_id.saturating_sub(1)),
                    Just(largest.group_id.saturating_add(1)),
                    id_near_bounds(),
                ];
                (Just(largest), near)
            }),
            object_id in id_near_bounds(),
        ) {
            // Arrange
            let mut control = DeliveryControl::new(128, true, &FilterType::LargestObject, None);
            control.apply_update(control.requested(128, true, None, Some(largest)));
            let model = [Some(object_id), largest.object_id.checked_add(1)]
                .into_iter()
                .flatten()
                .filter(|&id| id >= object_id && location(group_id, id) > largest)
                .min();

            // Act
            let resumed = control.resume_object_id(group_id, object_id);

            // Assert
            prop_assert_eq!(resumed, model);
        }
    }

    #[test]
    fn turning_forward_on_counts_a_resume() {
        // Arrange
        let mut control = DeliveryControl::new(128, true, &FilterType::LargestObject, None);

        // Act
        control.apply_update(control.requested(128, false, None, None));
        let paused = control.resumes;
        control.apply_update(control.requested(128, true, None, Some(location(4, 2))));

        // Assert
        assert_eq!((paused, control.resumes), (0, 1));
    }

    #[test]
    fn an_update_without_a_filter_keeps_the_range_and_takes_forward() {
        // Arrange
        let control = bounded(location(3, 0), 8);

        // Act
        let requested = control.requested(128, false, None, Some(location(5, 2)));

        // Assert
        assert!(!control.widens_to(&requested));
        assert_eq!(
            (requested.forward, requested.start, requested.end_group),
            (false, Some(location(3, 0)), Some(8))
        );
        assert_eq!(requested.largest_at_update, Some(location(5, 2)));
    }

    #[test]
    fn an_update_takes_the_subscriber_priority_and_keeps_the_range() {
        // Arrange
        let mut control = bounded(location(3, 0), 8);

        // Act
        control.apply_update(control.requested(7, true, None, None));

        // Assert
        assert_eq!(control.subscriber_priority, 7);
        assert_eq!(
            (control.start, control.end_group),
            (Some(location(3, 0)), Some(8))
        );
        assert_eq!((control.restarts, control.resumes), (0, 0));
    }

    #[test]
    fn a_relative_filter_in_an_update_starts_from_the_current_largest_object() {
        // Arrange
        let control =
            DeliveryControl::new(128, true, &FilterType::LargestObject, Some(location(1, 0)));

        // Act
        let requested = control.requested(
            128,
            true,
            Some(FilterType::NextGroupStart),
            Some(location(5, 2)),
        );

        // Assert
        assert_eq!(requested.start, Some(location(6, 0)));
    }
}
