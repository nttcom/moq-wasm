use moqt::{object::Location, subscribe::FilterType};

/// The part of a downstream subscription a SUBSCRIBE_UPDATE / REQUEST_UPDATE
/// can change (draft-14 §9.10, draft-18 §10.9.1), shared by its registration
/// and its egress runner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DeliveryControl {
    pub(crate) forward: bool,
    /// The subscription's Start Location (§9.7), before egress raises it
    /// past the Largest Object.
    pub(crate) start: Location,
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
}

impl DeliveryControl {
    /// `largest` is the Largest Location the subscription was accepted with.
    /// An AbsoluteRange End Group is the last group included (§9.7).
    pub(crate) fn new(forward: bool, filter_type: &FilterType, largest: Option<Location>) -> Self {
        let end_group = match filter_type {
            FilterType::AbsoluteRange { end_group, .. } => Some(end_group.saturating_add(1)),
            _ => None,
        };
        Self {
            forward,
            start: filter_type.start_location(largest),
            end_group,
            largest_at_update: largest,
            restarts: 0,
            resumes: 0,
        }
    }

    /// `end_group` is the SUBSCRIBE_UPDATE field: the end group plus 1, with
    /// 0 meaning open-ended (§9.10).
    #[cfg(test)]
    pub(crate) fn from_subscribe_update(forward: bool, start: Location, end_group: u64) -> Self {
        Self {
            forward,
            start,
            end_group: (end_group != 0).then_some(end_group),
            largest_at_update: None,
            restarts: 0,
            resumes: 0,
        }
    }

    /// What an update asks for when the track's Largest Object is `largest`:
    /// its Forward State and the range of `filter`, resolved like a
    /// SUBSCRIBE's; `None` keeps the current range.
    pub(crate) fn requested(
        &self,
        forward: bool,
        filter: Option<FilterType>,
        largest: Option<Location>,
    ) -> Self {
        let range = match filter {
            Some(filter) => Self::new(forward, &filter, largest),
            None => *self,
        };
        Self {
            forward,
            largest_at_update: largest,
            restarts: self.restarts,
            resumes: self.resumes,
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
        requested.start < self.start || end_moves_out
    }

    pub(crate) fn contains_group(&self, group_id: u64) -> bool {
        group_id >= self.start.group_id && !self.is_past_end(group_id)
    }

    pub(crate) fn is_past_end(&self, group_id: u64) -> bool {
        self.end_group.is_some_and(|end| group_id >= end)
    }

    /// Takes the Forward State and range of `requested`, which the session's
    /// draft has already allowed (see `widens_to`).
    pub(crate) fn apply_update(&mut self, requested: DeliveryControl) {
        let restarts = self.restarts + u64::from(requested.start < self.start);
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
            Some(largest) if group_id == largest.group_id => {
                Some(object_id.max(largest.object_id.saturating_add(1)))
            }
            _ => Some(object_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::test_support::relay_harness::fixtures::location;

    fn bounded(start: Location, end_group: u64) -> DeliveryControl {
        DeliveryControl::from_subscribe_update(true, start, end_group)
    }

    #[test]
    fn start_follows_the_filter_and_the_largest_location() {
        // Arrange
        let largest = Some(location(5, 3));

        // Act
        let control = DeliveryControl::new(true, &FilterType::NextGroupStart, largest);

        // Assert
        assert_eq!(control.start, location(6, 0));
        assert_eq!(control.end_group, None);
    }

    #[test]
    fn absolute_range_includes_its_end_group() {
        // Arrange
        let filter_type = FilterType::AbsoluteRange {
            location: location(2, 0),
            end_group: 4,
        };

        // Act
        let control = DeliveryControl::new(true, &filter_type, None);

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
        let mut control = DeliveryControl::new(true, &FilterType::LargestObject, None);
        let filter = FilterType::AbsoluteRange {
            location: location(3, 1),
            end_group: 7,
        };
        let requested = control.requested(true, Some(filter), None);

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
        let start_back = control.requested(true, Some(earlier_start), None);
        let end_out = control.requested(true, Some(open_end), None);

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
        control.apply_update(control.requested(true, Some(later_start), None));
        let after_narrowing = control.restarts;
        control.apply_update(control.requested(true, Some(earlier_start), None));

        // Assert
        assert_eq!((after_narrowing, control.restarts), (0, 1));
    }

    #[test]
    fn a_subgroup_continues_past_the_largest_object_at_the_update() {
        // Arrange
        let mut control = DeliveryControl::new(true, &FilterType::LargestObject, None);

        // Act
        control.apply_update(control.requested(true, None, Some(location(4, 2))));

        // Assert
        assert_eq!(control.resume_object_id(3, 7), None);
        assert_eq!(control.resume_object_id(4, 1), Some(3));
        assert_eq!(control.resume_object_id(4, 5), Some(5));
        assert_eq!(control.resume_object_id(5, 0), Some(0));
    }

    #[test]
    fn turning_forward_on_counts_a_resume() {
        // Arrange
        let mut control = DeliveryControl::new(true, &FilterType::LargestObject, None);

        // Act
        control.apply_update(control.requested(false, None, None));
        let paused = control.resumes;
        control.apply_update(control.requested(true, None, Some(location(4, 2))));

        // Assert
        assert_eq!((paused, control.resumes), (0, 1));
    }

    #[test]
    fn an_update_without_a_filter_keeps_the_range_and_takes_forward() {
        // Arrange
        let control = bounded(location(3, 0), 8);

        // Act
        let requested = control.requested(false, None, Some(location(5, 2)));

        // Assert
        assert!(!control.widens_to(&requested));
        assert_eq!(
            (requested.forward, requested.start, requested.end_group),
            (false, location(3, 0), Some(8))
        );
        assert_eq!(requested.largest_at_update, Some(location(5, 2)));
    }

    #[test]
    fn a_relative_filter_in_an_update_starts_from_the_current_largest_object() {
        // Arrange
        let control = DeliveryControl::new(true, &FilterType::LargestObject, Some(location(1, 0)));

        // Act
        let requested =
            control.requested(true, Some(FilterType::NextGroupStart), Some(location(5, 2)));

        // Assert
        assert_eq!(requested.start, location(6, 0));
    }
}
