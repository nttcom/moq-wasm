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
    /// `end` is the End Location of FETCH_OK (draft-14 §9.17).
    pub(crate) fn resolve(
        start: Location,
        requested_end: Location,
        largest: Option<Location>,
    ) -> Result<Self, RequestRejection> {
        if ends_at_or_before(start, requested_end) {
            return Err(RequestRejection::InvalidRange);
        }
        let Some(largest) = largest else {
            return Err(RequestRejection::InvalidRange);
        };
        if start > largest {
            return Err(RequestRejection::InvalidRange);
        }
        Ok(Self {
            start,
            end: end_location(requested_end, largest),
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FetchTarget {
    pub(crate) track_key: TrackKey,
    pub(crate) start: Location,
    pub(crate) requested_end: Location,
}

impl FetchTarget {
    /// draft-14 §9.16.2.1: a Joining Fetch ends right after the Largest
    /// Location of the joined subscription and starts at Object 0 of
    /// `start_group`.
    pub(crate) fn joining(track_key: TrackKey, largest: Location, start_group: u64) -> Self {
        Self {
            track_key,
            start: Location {
                group_id: start_group,
                object_id: 0,
            },
            requested_end: location_after(largest),
        }
    }
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
    let covers_largest = if requested_end.object_id == 0 {
        requested_end.group_id >= largest.group_id
    } else {
        requested_end > after_largest
    };
    if covers_largest {
        after_largest
    } else {
        requested_end
    }
}

fn location_after(location: Location) -> Location {
    Location {
        group_id: location.group_id,
        object_id: location.object_id + 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(group_id: u64, object_id: u64) -> Location {
        Location {
            group_id,
            object_id,
        }
    }

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

    #[test]
    fn a_joining_target_ends_right_after_the_joined_largest_location() {
        // Act
        let target = FetchTarget::joining(
            TrackKey::new(vec!["live".to_string()], "video".to_string()),
            location(7, 3),
            5,
        );

        // Assert
        assert_eq!(target.start, location(5, 0));
        assert_eq!(target.requested_end, location(7, 4));
    }
}
