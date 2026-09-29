use moqt::wire::{GroupOrder, Location};

use crate::TrackKey;
use crate::request_rejection::RequestRejection;

/// Both ends use the FETCH End Location encoding (draft-14 §9.16.1): the
/// Location after the last Object, where Object 0 stands for the whole Group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FetchRange {
    pub(crate) start: Location,
    pub(crate) end: Location,
}

impl FetchRange {
    pub(crate) fn resolve(
        start: Location,
        requested_end: Location,
        largest: Option<Location>,
    ) -> Result<Self, RequestRejection> {
        match largest {
            Some(largest) if start <= largest && !ends_at_or_before(start, requested_end) => {
                Ok(Self {
                    start,
                    end: end_location(requested_end, largest),
                })
            }
            _ => Err(RequestRejection::InvalidRange),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FetchTarget {
    pub(crate) track_key: TrackKey,
    pub(crate) start: Location,
    pub(crate) requested_end: Location,
}

#[derive(Debug, Clone)]
pub(crate) struct IncomingFetchRequest {
    pub(crate) track_key: TrackKey,
    pub(crate) group_order: GroupOrder,
    pub(crate) range: FetchRange,
}

fn ends_at_or_before(start: Location, end: Location) -> bool {
    start.group_id > end.group_id
        || (start.group_id == end.group_id
            && end.object_id != 0
            && start.object_id >= end.object_id)
}

fn end_location(requested_end: Location, largest: Location) -> Location {
    let after_largest = location_after(largest);
    let whole_largest_group = Location {
        group_id: largest.group_id,
        object_id: 0,
    };
    if requested_end == whole_largest_group {
        after_largest
    } else {
        requested_end.min(after_largest)
    }
}

pub(crate) fn location_after(location: Location) -> Location {
    Location {
        group_id: location.group_id,
        object_id: location.object_id + 1,
    }
}

#[cfg(test)]
pub(crate) fn location(group_id: u64, object_id: u64) -> Location {
    Location {
        group_id,
        object_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_ending_before_the_largest_object_keeps_its_end() {
        // Act
        let range = FetchRange::resolve(location(2, 0), location(3, 4), Some(location(5, 1)));

        // Assert
        assert_eq!(range.unwrap().end, location(3, 4));
    }

    #[test]
    fn a_whole_group_before_the_largest_group_keeps_its_end() {
        // Act
        let range = FetchRange::resolve(location(2, 0), location(4, 0), Some(location(5, 1)));

        // Assert
        assert_eq!(range.unwrap().end, location(4, 0));
    }

    #[test]
    fn a_range_past_the_largest_object_ends_after_it() {
        // Act
        let range = FetchRange::resolve(location(2, 0), location(9, 3), Some(location(5, 1)));

        // Assert
        assert_eq!(range.unwrap().end, location(5, 2));
    }

    #[test]
    fn the_whole_largest_group_ends_after_the_largest_object() {
        // Act
        let range = FetchRange::resolve(location(2, 0), location(5, 0), Some(location(5, 1)));

        // Assert
        assert_eq!(range.unwrap().end, location(5, 2));
    }

    #[test]
    fn a_range_ending_right_after_the_largest_object_keeps_its_end() {
        // Act
        let range = FetchRange::resolve(location(5, 0), location(5, 2), Some(location(5, 1)));

        // Assert
        assert_eq!(range.unwrap().end, location(5, 2));
    }

    #[test]
    fn a_track_without_objects_is_an_invalid_range() {
        // Act
        let range = FetchRange::resolve(location(0, 0), location(1, 0), None);

        // Assert
        assert_eq!(range, Err(RequestRejection::InvalidRange));
    }

    #[test]
    fn a_start_past_the_largest_object_is_an_invalid_range() {
        // Act
        let range = FetchRange::resolve(location(5, 2), location(6, 0), Some(location(5, 1)));

        // Assert
        assert_eq!(range, Err(RequestRejection::InvalidRange));
    }

    #[test]
    fn an_end_at_the_start_is_an_invalid_range() {
        // Act
        let range = FetchRange::resolve(location(3, 2), location(3, 2), Some(location(5, 1)));

        // Assert
        assert_eq!(range, Err(RequestRejection::InvalidRange));
    }

    #[test]
    fn an_end_in_an_earlier_group_is_an_invalid_range() {
        // Act
        let range = FetchRange::resolve(location(3, 0), location(2, 0), Some(location(5, 1)));

        // Assert
        assert_eq!(range, Err(RequestRejection::InvalidRange));
    }
}
