//! `proptest` strategies over a small domain of Locations, for properties of
//! the cache's known ranges and FETCH plans checked against a brute-force
//! model.
//!
//! Each group has the Object IDs 0 to 5, a stand-in for every ID from 6 to
//! `u64::MAX - 1`, and `u64::MAX`. No generated bound falls strictly inside
//! `6..u64::MAX`, so the positions the stand-in holds are all known or all
//! unknown, and a model over the stand-ins decides every position.

use moqt::object::Location;
use proptest::{prelude::*, sample::select};

use crate::modules::test_support::relay_harness::fixtures::location;

/// The stand-in for every Object ID from 6 to `u64::MAX - 1`.
const MIDDLE: u64 = 6;

const OBJECT_IDS: [u64; 8] = [0, 1, 2, 3, 4, 5, MIDDLE, u64::MAX];

/// Every stand-in position of the groups below `groups`, in Location order.
pub(crate) fn positions(groups: u64) -> Vec<Location> {
    (0..groups)
        .flat_map(|group_id| OBJECT_IDS.map(|object_id| location(group_id, object_id)))
        .collect()
}

/// The last position `stand_in` holds.
pub(crate) fn last_held(stand_in: Location) -> Location {
    match stand_in.object_id {
        MIDDLE => location(stand_in.group_id, u64::MAX - 1),
        _ => stand_in,
    }
}

/// A position an object can be at in the groups below `groups`: Object ID
/// 0 to 5 or `u64::MAX`.
pub(crate) fn object_location(groups: u64) -> impl Strategy<Value = Location> {
    (
        0..groups,
        select(
            OBJECT_IDS
                .into_iter()
                .filter(|&object_id| object_id != MIDDLE)
                .collect::<Vec<_>>(),
        ),
    )
        .prop_map(|(group_id, object_id)| location(group_id, object_id))
}

/// A stand-in position of the groups below `groups`.
pub(crate) fn stand_in(groups: u64) -> impl Strategy<Value = Location> {
    (0..groups, select(OBJECT_IDS.to_vec()))
        .prop_map(|(group_id, object_id)| location(group_id, object_id))
}

/// The bound of a half-open range in the groups below `groups`: a stand-in
/// or, as an exclusive end, the first position after them.
pub(crate) fn bound(groups: u64) -> impl Strategy<Value = Location> {
    prop_oneof![9 => stand_in(groups), 1 => Just(location(groups, 0))]
}
