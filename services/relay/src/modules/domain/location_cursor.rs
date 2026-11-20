use moqt::object::Location;

/// A position in a track: before a Location, or past the last one. Ends
/// half-open ranges of Locations, so a range through group `u64::MAX`
/// (draft-18 Group IDs are vi64) has an end. Variant order puts `PastLast`
/// after every `At`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum LocationCursor {
    /// Before this Location.
    At(Location),
    /// After [`Location::MAX`].
    PastLast,
}

impl LocationCursor {
    /// Right after `at`: `PastLast` after [`Location::MAX`].
    pub(crate) fn after(at: Location) -> Self {
        at.next().map_or(Self::PastLast, Self::At)
    }

    /// Right after the last Object ID of `group_id`.
    pub(crate) fn after_group(group_id: u64) -> Self {
        Self::after(Location {
            group_id,
            object_id: u64::MAX,
        })
    }

    /// The exclusive end of a FETCH range with End Location `end`, where
    /// Object ID 0 means the whole group (draft-18 §10.12.1).
    pub(crate) fn fetch_end(end: Location) -> Self {
        if end.object_id == 0 {
            Self::after_group(end.group_id)
        } else {
            Self::At(end)
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::modules::test_support::{
        location_strategies::id_near_bounds, relay_harness::fixtures::location,
    };

    fn any_location() -> impl Strategy<Value = Location> {
        (id_near_bounds(), id_near_bounds())
            .prop_map(|(group_id, object_id)| location(group_id, object_id))
    }

    #[test]
    fn the_cursor_after_the_last_location_is_past_every_location() {
        // Act
        let past = LocationCursor::after(Location::MAX);

        // Assert
        assert_eq!(past, LocationCursor::PastLast);
        assert_eq!(LocationCursor::fetch_end(location(u64::MAX, 0)), past);
        assert!(LocationCursor::At(Location::MAX) < past);
    }

    proptest! {
        #[test]
        fn the_cursor_after_a_location_stands_before_exactly_the_locations_above_it(
            a in any_location(),
            b in any_location(),
        ) {
            // Act / Assert
            prop_assert_eq!(LocationCursor::At(a).cmp(&LocationCursor::At(b)), a.cmp(&b));
            prop_assert_eq!(LocationCursor::At(b) < LocationCursor::after(a), b <= a);
        }

        #[test]
        fn a_fetch_end_stands_before_exactly_the_locations_the_fetch_covers(
            end in any_location(),
            at in any_location(),
        ) {
            // Arrange: Object ID 0 covers the whole group, any other stops before it
            let covered = if end.object_id == 0 {
                at.group_id <= end.group_id
            } else {
                at < end
            };

            // Act / Assert
            prop_assert_eq!(LocationCursor::At(at) < LocationCursor::fetch_end(end), covered);
        }
    }
}
