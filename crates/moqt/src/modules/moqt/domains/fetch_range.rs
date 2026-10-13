use thiserror::Error;

use crate::object::Location;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum FetchRangeError {
    #[error("the range ends at or before its start")]
    EmptyRange,
    #[error("the track has no objects yet")]
    NothingPublished,
    #[error("the range starts after the largest object")]
    StartAfterLargest,
}

/// The End Location a FETCH_OK answers to a Standalone FETCH for
/// `[start, requested_end)` on a track whose largest object is `largest`.
/// draft-14 §9.16.1: an end object id of 0 requests the whole group. §9.17: the
/// response ends after the largest object when the request reaches past it, at
/// the whole group when a group below the largest was requested in full, and
/// at the request otherwise.
pub fn resolve_fetch_range(
    start: Location,
    requested_end: Location,
    largest: Option<Location>,
) -> Result<Location, FetchRangeError> {
    if explicit_end_before_or_equal_start(start, requested_end) {
        return Err(FetchRangeError::EmptyRange);
    }
    let Some(largest) = largest else {
        return Err(FetchRangeError::NothingPublished);
    };
    if start > largest {
        return Err(FetchRangeError::StartAfterLargest);
    }
    Ok(response_end_location(requested_end, largest))
}

fn explicit_end_before_or_equal_start(start: Location, end: Location) -> bool {
    start.group_id > end.group_id
        || (start.group_id == end.group_id
            && end.object_id != 0
            && start.object_id >= end.object_id)
}

fn response_end_location(requested_end: Location, largest: Location) -> Location {
    let after_largest = largest.next();
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
        // Act / Assert
        assert_eq!(
            resolve_fetch_range(location(2, 0), location(3, 4), Some(location(5, 1))),
            Ok(location(3, 4))
        );
    }

    #[test]
    fn a_whole_group_before_the_largest_group_keeps_its_end() {
        // Act / Assert
        assert_eq!(
            resolve_fetch_range(location(2, 0), location(4, 0), Some(location(5, 1))),
            Ok(location(4, 0))
        );
    }

    #[test]
    fn a_range_past_the_largest_object_ends_after_it() {
        // Act / Assert
        assert_eq!(
            resolve_fetch_range(location(2, 0), location(9, 3), Some(location(5, 1))),
            Ok(location(5, 2))
        );
        assert_eq!(
            resolve_fetch_range(location(5, 0), location(5, 7), Some(location(5, 1))),
            Ok(location(5, 2))
        );
    }

    #[test]
    fn the_whole_largest_group_ends_after_the_largest_object() {
        // Act / Assert
        assert_eq!(
            resolve_fetch_range(location(2, 0), location(5, 0), Some(location(5, 1))),
            Ok(location(5, 2))
        );
    }

    #[test]
    fn a_range_ending_right_after_the_largest_object_keeps_its_end() {
        // Act / Assert
        assert_eq!(
            resolve_fetch_range(location(5, 0), location(5, 2), Some(location(5, 1))),
            Ok(location(5, 2))
        );
    }

    #[test]
    fn a_track_without_objects_has_nothing_published() {
        // Act / Assert
        assert_eq!(
            resolve_fetch_range(location(0, 0), location(1, 0), None),
            Err(FetchRangeError::NothingPublished)
        );
    }

    #[test]
    fn a_start_past_the_largest_object_is_rejected() {
        // Act / Assert
        assert_eq!(
            resolve_fetch_range(location(5, 2), location(6, 0), Some(location(5, 1))),
            Err(FetchRangeError::StartAfterLargest)
        );
        assert_eq!(
            resolve_fetch_range(location(5, 1), location(6, 0), Some(location(5, 1))),
            Ok(location(5, 2))
        );
    }

    #[test]
    fn an_end_at_or_before_the_start_is_empty_whatever_the_track_holds() {
        // Act / Assert
        assert_eq!(
            resolve_fetch_range(location(3, 2), location(3, 2), Some(location(5, 1))),
            Err(FetchRangeError::EmptyRange)
        );
        assert_eq!(
            resolve_fetch_range(location(3, 0), location(2, 0), Some(location(5, 1))),
            Err(FetchRangeError::EmptyRange)
        );
        assert_eq!(
            resolve_fetch_range(location(3, 2), location(3, 1), None),
            Err(FetchRangeError::EmptyRange)
        );
        assert_eq!(
            resolve_fetch_range(location(3, 2), location(3, 0), Some(location(5, 1))),
            Ok(location(3, 0))
        );
    }
}
