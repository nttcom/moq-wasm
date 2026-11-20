//! `proptest` strategies over a small domain of Locations, for properties of
//! the cache's known ranges and FETCH plans checked against a brute-force
//! model.
//!
//! Each group has the Object IDs 0 to 5, a stand-in for every ID from 6 to
//! `u64::MAX - 1`, and `u64::MAX`. No generated bound falls strictly inside
//! `6..u64::MAX`, so the positions the stand-in holds are all known or all
//! unknown, and a model over the stand-ins decides every position. The groups
//! start at Group ID 0 or end at `u64::MAX`, the largest a draft-18 publisher
//! can send.

use std::ops::RangeInclusive;

use moqt::object::Location;
use proptest::{prelude::*, sample::select};

use crate::modules::{
    domain::location_cursor::LocationCursor, test_support::relay_harness::fixtures::location,
};

/// The stand-in for every Object ID from 6 to `u64::MAX - 1`.
const MIDDLE: u64 = 6;

const OBJECT_IDS: [u64; 8] = [0, 1, 2, 3, 4, 5, MIDDLE, u64::MAX];

pub(crate) fn id_near_bounds() -> impl Strategy<Value = u64> {
    prop_oneof![
        Just(0),
        Just(1),
        Just(u64::MAX - 1),
        Just(u64::MAX),
        0..8u64,
        any::<u64>(),
    ]
}

/// `count` consecutive Group IDs a property runs over.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Groups {
    first: u64,
    count: u64,
}

/// `count` groups from Group ID 0 or through Group ID `u64::MAX`.
pub(crate) fn groups(count: u64) -> impl Strategy<Value = Groups> {
    prop_oneof![
        Just(Groups { first: 0, count }),
        Just(Groups {
            first: u64::MAX - (count - 1),
            count,
        }),
    ]
}

impl Groups {
    fn ids(self) -> RangeInclusive<u64> {
        self.first..=self.first + (self.count - 1)
    }

    /// Every stand-in position, in Location order.
    pub(crate) fn positions(self) -> Vec<Location> {
        self.ids()
            .flat_map(|group_id| OBJECT_IDS.map(|object_id| location(group_id, object_id)))
            .collect()
    }

    /// The first position after the groups.
    fn end(self) -> LocationCursor {
        LocationCursor::after_group(*self.ids().end())
    }

    /// Every stand-in position as a bound, then the first position after
    /// them.
    pub(crate) fn bounds(self) -> Vec<LocationCursor> {
        let mut bounds: Vec<LocationCursor> = self
            .positions()
            .into_iter()
            .map(LocationCursor::At)
            .collect();
        bounds.push(self.end());
        bounds
    }

    /// A position an object can be at: Object ID 0 to 5 or `u64::MAX`.
    pub(crate) fn object_location(self) -> impl Strategy<Value = Location> {
        (
            self.ids(),
            select(
                OBJECT_IDS
                    .into_iter()
                    .filter(|&object_id| object_id != MIDDLE)
                    .collect::<Vec<_>>(),
            ),
        )
            .prop_map(|(group_id, object_id)| location(group_id, object_id))
    }

    /// A stand-in position.
    pub(crate) fn stand_in(self) -> impl Strategy<Value = Location> {
        (self.ids(), select(OBJECT_IDS.to_vec()))
            .prop_map(|(group_id, object_id)| location(group_id, object_id))
    }

    /// The bound of a half-open range: a stand-in or, as an exclusive end,
    /// the first position after the groups.
    pub(crate) fn bound(self) -> impl Strategy<Value = LocationCursor> {
        prop_oneof![
            9 => self.stand_in().prop_map(LocationCursor::At),
            1 => Just(self.end()),
        ]
    }
}

/// The last position `stand_in` holds.
pub(crate) fn last_held(stand_in: Location) -> Location {
    match stand_in.object_id {
        MIDDLE => location(stand_in.group_id, u64::MAX - 1),
        _ => stand_in,
    }
}
