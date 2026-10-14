use moqt::{object::Location, subscribe::FilterType};

/// The part of a downstream subscription a SUBSCRIBE_UPDATE can change
/// (draft-14 §9.10), shared by its registration and its egress runner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DeliveryControl {
    pub(crate) forward: bool,
    /// The subscription's Start Location (§9.7), before egress raises it
    /// past the subscribe-time Largest Object.
    pub(crate) start: Location,
    /// The first group past the subscription; `None` is open-ended.
    pub(crate) end_group: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateOutcome {
    Applied,
    /// §9.10 only lets an update narrow the range; the widening Start
    /// Location or End Group kept its previous value.
    WideningIgnored,
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
        }
    }

    /// `end_group` is the SUBSCRIBE_UPDATE field: the end group plus 1, with
    /// 0 meaning open-ended (§9.10).
    pub(crate) fn from_subscribe_update(forward: bool, start: Location, end_group: u64) -> Self {
        Self {
            forward,
            start,
            end_group: (end_group != 0).then_some(end_group),
        }
    }

    pub(crate) fn contains_group(&self, group_id: u64) -> bool {
        group_id >= self.start.group_id && !self.is_past_end(group_id)
    }

    pub(crate) fn is_past_end(&self, group_id: u64) -> bool {
        self.end_group.is_some_and(|end| group_id >= end)
    }

    /// Takes the Forward State of `update` and the narrowing part of its range.
    pub(crate) fn apply_update(&mut self, update: DeliveryControl) -> UpdateOutcome {
        let start_widens = update.start < self.start;
        let end_widens = match (self.end_group, update.end_group) {
            (Some(current), Some(requested)) => requested > current,
            (Some(_), None) => true,
            (None, _) => false,
        };
        self.forward = update.forward;
        self.start = self.start.max(update.start);
        self.end_group = match (self.end_group, update.end_group) {
            (Some(current), Some(requested)) => Some(current.min(requested)),
            (current, requested) => current.or(requested),
        };
        if start_widens || end_widens {
            UpdateOutcome::WideningIgnored
        } else {
            UpdateOutcome::Applied
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

        // Act
        let outcome = control.apply_update(bounded(location(3, 1), 8));

        // Assert
        assert_eq!(outcome, UpdateOutcome::Applied);
        assert_eq!(control, bounded(location(3, 1), 8));
    }

    #[test]
    fn widening_update_keeps_the_previous_range_but_takes_forward() {
        // Arrange
        let mut control = bounded(location(3, 0), 8);
        let update = DeliveryControl::from_subscribe_update(false, location(1, 0), 0);

        // Act
        let outcome = control.apply_update(update);

        // Assert
        assert_eq!(outcome, UpdateOutcome::WideningIgnored);
        assert_eq!(
            control,
            DeliveryControl::from_subscribe_update(false, location(3, 0), 8)
        );
    }

    #[test]
    fn partly_widening_update_applies_its_narrowing_end() {
        // Arrange
        let mut control = bounded(location(3, 0), 8);

        // Act
        let outcome = control.apply_update(bounded(location(2, 0), 6));

        // Assert: the lower start is ignored, the smaller end applied
        assert_eq!(outcome, UpdateOutcome::WideningIgnored);
        assert_eq!(control, bounded(location(3, 0), 6));
    }
}
