use thiserror::Error;

use crate::{RequestErrorCode, object::Location, subscribe::FilterType};

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

/// What a Joining Fetch of a subscription resolves against (draft-14 §9.16.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinPoint {
    /// The Largest Location of the SUBSCRIBE_OK; `None` when Content Exists
    /// was 0.
    Largest(Option<Location>),
    NotLargestObject,
}

impl JoinPoint {
    pub fn new(filter_type: &FilterType, largest: Option<Location>) -> Self {
        match filter_type {
            FilterType::LargestObject => Self::Largest(largest),
            _ => Self::NotLargestObject,
        }
    }
}

/// The Joining Start of a Relative or Absolute Joining Fetch (draft-14
/// §9.16.2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoiningStart {
    /// Groups before the joined Largest Location's group.
    Relative(u64),
    Absolute(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum JoiningFetchError {
    #[error("no subscription has the joining request id")]
    UnknownSubscription,
    #[error("the joined subscription does not use the Largest Object filter")]
    NotLargestObject,
    #[error("the track had no objects when the joined subscription was answered")]
    NothingPublished,
    #[error("the range starts after the joined largest object")]
    StartAfterLargest,
}

impl JoiningFetchError {
    /// The FETCH_ERROR code; `None` when the session closes with
    /// PROTOCOL_VIOLATION instead (draft-14 §9.16.2).
    pub fn request_error_code(self) -> Option<RequestErrorCode> {
        match self {
            Self::UnknownSubscription => Some(RequestErrorCode::InvalidJoiningRequestId),
            Self::NotLargestObject => None,
            Self::NothingPublished | Self::StartAfterLargest => {
                Some(RequestErrorCode::InvalidRange)
            }
        }
    }
}

/// The Start and End Location of a Joining Fetch, in the Standalone Fetch
/// encoding: from Object 0 of the start group to right after the joined
/// Largest Location (draft-14 §9.16.2.1). The caller answers
/// `JoiningFetchError::UnknownSubscription` when the session has no such
/// subscription.
pub fn resolve_joining_fetch(
    joining_start: JoiningStart,
    joined: JoinPoint,
) -> Result<(Location, Location), JoiningFetchError> {
    let largest = match joined {
        JoinPoint::NotLargestObject => return Err(JoiningFetchError::NotLargestObject),
        JoinPoint::Largest(None) => return Err(JoiningFetchError::NothingPublished),
        JoinPoint::Largest(Some(largest)) => largest,
    };
    let start_group_id = match joining_start {
        JoiningStart::Relative(groups) => largest.group_id.saturating_sub(groups),
        JoiningStart::Absolute(group_id) => group_id,
    };
    let start = Location {
        group_id: start_group_id,
        object_id: 0,
    };
    if start > largest {
        return Err(JoiningFetchError::StartAfterLargest);
    }
    Ok((start, largest.next()))
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
    use crate::modules::test_support::location;

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

    #[test]
    fn a_relative_joining_fetch_starts_groups_before_the_joined_largest_group() {
        // Act / Assert
        assert_eq!(
            resolve_joining_fetch(
                JoiningStart::Relative(2),
                JoinPoint::Largest(Some(location(5, 3)))
            ),
            Ok((location(3, 0), location(5, 4)))
        );
        assert_eq!(
            resolve_joining_fetch(
                JoiningStart::Relative(9),
                JoinPoint::Largest(Some(location(5, 3)))
            ),
            Ok((location(0, 0), location(5, 4)))
        );
    }

    #[test]
    fn an_absolute_joining_fetch_starts_at_the_requested_group() {
        // Act / Assert
        assert_eq!(
            resolve_joining_fetch(
                JoiningStart::Absolute(5),
                JoinPoint::Largest(Some(location(5, 3)))
            ),
            Ok((location(5, 0), location(5, 4)))
        );
        assert_eq!(
            resolve_joining_fetch(
                JoiningStart::Absolute(6),
                JoinPoint::Largest(Some(location(5, 3)))
            ),
            Err(JoiningFetchError::StartAfterLargest)
        );
    }

    #[test]
    fn a_joining_fetch_without_a_largest_object_subscription_is_rejected() {
        // Act / Assert
        assert_eq!(
            resolve_joining_fetch(JoiningStart::Relative(0), JoinPoint::NotLargestObject),
            Err(JoiningFetchError::NotLargestObject)
        );
        assert_eq!(
            resolve_joining_fetch(JoiningStart::Relative(0), JoinPoint::Largest(None)),
            Err(JoiningFetchError::NothingPublished)
        );
    }

    #[test]
    fn only_a_joined_subscription_without_the_largest_object_filter_closes_the_session() {
        // Act / Assert
        assert_eq!(
            JoiningFetchError::UnknownSubscription.request_error_code(),
            Some(RequestErrorCode::InvalidJoiningRequestId)
        );
        assert_eq!(
            JoiningFetchError::NotLargestObject.request_error_code(),
            None
        );
        assert_eq!(
            JoiningFetchError::NothingPublished.request_error_code(),
            Some(RequestErrorCode::InvalidRange)
        );
        assert_eq!(
            JoiningFetchError::StartAfterLargest.request_error_code(),
            Some(RequestErrorCode::InvalidRange)
        );
    }

    mod model {
        use std::collections::BTreeSet;

        use proptest::prelude::*;

        use super::*;

        /// Whether `[start, end)` names `at`, read from the draft text: End
        /// Location is the last object plus 1, and an Object of 0 names the
        /// entire group (draft-14 §9.16.1, draft-18 §10.12.1); the range is
        /// inclusive of the last object (draft-14 §9.16.3).
        fn named(at: Location, start: Location, end: Location) -> bool {
            let up_to_last = if end.object_id == 0 {
                at.group_id <= end.group_id
            } else {
                let last = Location {
                    group_id: end.group_id,
                    object_id: end.object_id - 1,
                };
                at <= last
            };
            at >= start && up_to_last
        }

        /// The locations next to the given ones, where `named` can change.
        fn grid(locations: &[Location]) -> Vec<Location> {
            let near = |value: u64| {
                [
                    value.saturating_sub(1),
                    value,
                    value.saturating_add(1),
                    0,
                    1,
                    u64::MAX,
                ]
            };
            let groups: BTreeSet<u64> = locations.iter().flat_map(|l| near(l.group_id)).collect();
            let objects: BTreeSet<u64> = locations.iter().flat_map(|l| near(l.object_id)).collect();
            groups
                .iter()
                .flat_map(|&group_id| {
                    objects.iter().map(move |&object_id| Location {
                        group_id,
                        object_id,
                    })
                })
                .collect()
        }

        fn coordinate() -> impl Strategy<Value = u64> {
            prop_oneof![
                0..4u64,
                Just(u64::MAX),
                Just(u64::MAX - 1),
                Just((1 << 62) - 1),
                any::<u64>(),
            ]
        }

        fn any_location() -> impl Strategy<Value = Location> {
            (coordinate(), coordinate()).prop_map(|(group_id, object_id)| Location {
                group_id,
                object_id,
            })
        }

        /// A location in the group before, of or after `anchor`, at object
        /// 0, 1, or one off `anchor`'s.
        fn near(anchor: Location) -> impl Strategy<Value = Location> {
            let group = prop_oneof![
                Just(anchor.group_id.saturating_sub(1)),
                Just(anchor.group_id),
                Just(anchor.group_id.saturating_add(1)),
            ];
            let object = prop_oneof![
                Just(0),
                Just(1),
                Just(anchor.object_id.saturating_sub(1)),
                Just(anchor.object_id),
                Just(anchor.object_id.saturating_add(1)),
                Just(u64::MAX),
            ];
            (group, object).prop_map(|(group_id, object_id)| Location {
                group_id,
                object_id,
            })
        }

        fn either_near(a: Location, b: Location) -> BoxedStrategy<Location> {
            prop_oneof![near(a), near(b), any_location()].boxed()
        }

        fn fetch() -> impl Strategy<Value = (Location, Location, Option<Location>)> {
            any_location()
                .prop_flat_map(|start| {
                    (
                        Just(start),
                        prop_oneof![3 => near(start), 1 => any_location()],
                    )
                })
                .prop_flat_map(|(start, end)| {
                    (
                        Just(start),
                        Just(end),
                        proptest::option::weighted(0.9, either_near(start, end)),
                    )
                })
        }

        proptest! {
            #[test]
            fn a_standalone_fetch_covers_the_named_objects_up_to_the_largest(
                (start, end, largest) in fetch(),
            ) {
                // Arrange
                let points = grid(&[start, end, largest.unwrap_or(start)]);

                // Act
                let resolved = resolve_fetch_range(start, end, largest);

                // Assert: an empty request, an empty track and a start past the largest fail in that order; otherwise the End Location names exactly the requested objects up to the largest
                let expected_error = if !named(start, start, end) {
                    Some(FetchRangeError::EmptyRange)
                } else if let Some(largest) = largest {
                    (start > largest).then_some(FetchRangeError::StartAfterLargest)
                } else {
                    Some(FetchRangeError::NothingPublished)
                };
                match (expected_error, resolved) {
                    (Some(expected), resolved) => prop_assert_eq!(resolved, Err(expected)),
                    (None, Err(error)) => prop_assert!(false, "unexpected {:?}", error),
                    (None, Ok(response_end)) => {
                        let largest = largest.unwrap();
                        for at in points {
                            prop_assert_eq!(
                                named(at, start, response_end),
                                named(at, start, end) && at <= largest,
                                "at {:?}, response end {:?}",
                                at,
                                response_end
                            );
                        }
                    }
                }
            }

            #[test]
            fn a_joining_fetch_covers_its_start_group_up_to_the_joined_largest(
                largest in any_location(),
                relative in any::<bool>(),
                groups in prop_oneof![0..3u64, Just(u64::MAX), any::<u64>()],
            ) {
                // Arrange
                let (joining_start, start_group_id) = if relative {
                    (JoiningStart::Relative(groups), largest.group_id.saturating_sub(groups))
                } else {
                    let group_id = largest.group_id.saturating_add(groups % 3).saturating_sub(1);
                    (JoiningStart::Absolute(group_id), group_id)
                };
                let start = Location { group_id: start_group_id, object_id: 0 };
                let points = grid(&[start, largest]);

                // Act
                let resolved = resolve_joining_fetch(joining_start, JoinPoint::Largest(Some(largest)));

                // Assert: the range runs from Object 0 of the start group through the joined Largest Location, and a start group past it fails
                if start_group_id > largest.group_id {
                    prop_assert_eq!(resolved, Err(JoiningFetchError::StartAfterLargest));
                } else {
                    prop_assert!(resolved.is_ok(), "unexpected {:?}", resolved);
                    let (resolved_start, resolved_end) = resolved.unwrap();
                    prop_assert_eq!(resolved_start, start);
                    for at in points {
                        prop_assert_eq!(
                            named(at, resolved_start, resolved_end),
                            at >= start && at <= largest,
                            "at {:?}, end {:?}",
                            at,
                            resolved_end
                        );
                    }
                }
            }
        }
    }
}
