use moqt::RequestErrorCode;
use moqt::{
    fetch::resolve_fetch_range,
    wire::{GroupOrder, Location},
};

use crate::client_state::TrackKey;

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
    ) -> Result<Self, RequestErrorCode> {
        let end = resolve_fetch_range(start, requested_end, largest)
            .map_err(|_| RequestErrorCode::InvalidRange)?;
        Ok(Self { start, end })
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
        assert_eq!(range, Err(RequestErrorCode::InvalidRange));
    }

    #[test]
    fn a_start_past_the_largest_object_is_an_invalid_range() {
        // Act
        let range = FetchRange::resolve(location(5, 2), location(6, 0), Some(location(5, 1)));

        // Assert
        assert_eq!(range, Err(RequestErrorCode::InvalidRange));
    }

    #[test]
    fn an_end_at_the_start_is_an_invalid_range() {
        // Act
        let range = FetchRange::resolve(location(3, 2), location(3, 2), Some(location(5, 1)));

        // Assert
        assert_eq!(range, Err(RequestErrorCode::InvalidRange));
    }

    #[test]
    fn an_end_in_an_earlier_group_is_an_invalid_range() {
        // Act
        let range = FetchRange::resolve(location(3, 0), location(2, 0), Some(location(5, 1)));

        // Assert
        assert_eq!(range, Err(RequestErrorCode::InvalidRange));
    }
}
