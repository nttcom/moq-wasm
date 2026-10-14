use moqt::{object::Location, subscribe::FilterType};

/// The part of a downstream subscription a SUBSCRIBE_UPDATE can change
/// (draft-14 §9.10), shared by its registration and its egress runner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DeliveryControl {
    pub(crate) forward: bool,
    /// The subscription's Start Location (§9.7), before egress raises it
    /// past the subscribe-time Largest Object.
    pub(crate) start: Location,
}

impl DeliveryControl {
    /// `largest` is the Largest Location the subscription was accepted with.
    pub(crate) fn new(forward: bool, filter_type: &FilterType, largest: Option<Location>) -> Self {
        Self {
            forward,
            start: filter_type.start_location(largest),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::test_support::relay_harness::fixtures::location;

    #[test]
    fn start_follows_the_filter_and_the_largest_location() {
        // Arrange
        let largest = Some(location(5, 3));

        // Act
        let control = DeliveryControl::new(true, &FilterType::NextGroupStart, largest);

        // Assert
        assert_eq!(control.start, location(6, 0));
    }
}
