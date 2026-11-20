use std::ops::Bound;

use moqt::object::Location;

/// Where a reader of one group stands. Orders by position, past the group
/// after every Object ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ObjectCursor {
    /// Before this Object ID, which is still to be read.
    At(u64),
    /// After Object ID `u64::MAX`, the group's last.
    PastGroup,
}

impl ObjectCursor {
    /// Right after `object_id`: `PastGroup` after `u64::MAX`.
    pub(crate) fn after(object_id: u64) -> Self {
        object_id.checked_add(1).map_or(Self::PastGroup, Self::At)
    }

    /// The first Object ID not yet read; `None` past the group.
    pub(crate) fn object_id(self) -> Option<u64> {
        match self {
            Self::At(object_id) => Some(object_id),
            Self::PastGroup => None,
        }
    }

    /// The Locations of `group_id` from the cursor to the end of the group,
    /// as `BTreeMap::range` bounds: empty past the group.
    pub(crate) fn rest_of_group(self, group_id: u64) -> (Bound<Location>, Bound<Location>) {
        let last = Location {
            group_id,
            object_id: u64::MAX,
        };
        let start = match self {
            Self::At(object_id) => Bound::Included(Location {
                group_id,
                object_id,
            }),
            Self::PastGroup => Bound::Excluded(last),
        };
        (start, Bound::Included(last))
    }
}

impl Default for ObjectCursor {
    fn default() -> Self {
        Self::At(0)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proptest::{collection::btree_set, prelude::*};

    use super::*;
    use crate::modules::test_support::{
        location_strategies::id_near_bounds, relay_harness::fixtures::location,
    };

    #[test]
    fn the_cursor_after_the_largest_object_id_is_past_the_group() {
        // Act
        let past = ObjectCursor::after(u64::MAX);

        // Assert
        assert_eq!(past.object_id(), None);
        assert!(ObjectCursor::At(u64::MAX) < past);
        let mut objects = BTreeMap::new();
        objects.insert(location(3, u64::MAX), ());
        assert_eq!(objects.range(past.rest_of_group(3)).count(), 0);
    }

    proptest! {
        #[test]
        fn a_reader_stepping_after_each_object_reads_every_object_once_in_order(
            object_ids in btree_set(id_near_bounds(), 0..8),
            start in id_near_bounds(),
        ) {
            // Arrange: neighbouring groups hold objects the reader must not reach
            let group_id = 5;
            let mut objects = BTreeMap::new();
            for &object_id in &object_ids {
                for group_id in [group_id - 1, group_id, group_id + 1] {
                    objects.insert(location(group_id, object_id), ());
                }
            }
            let model: Vec<u64> = object_ids.range(start..).copied().collect();

            // Act: as a send loop does, take the first object from the cursor and step after it
            let mut cursor = ObjectCursor::At(start);
            let mut read = Vec::new();
            let mut cursors = vec![cursor];
            while let Some((at, ())) = objects.range(cursor.rest_of_group(group_id)).next() {
                prop_assert!(read.len() < object_ids.len(), "read past every object: {read:?}");
                read.push(at.object_id);
                cursor = ObjectCursor::after(at.object_id);
                cursors.push(cursor);
            }

            // Assert: each step moves strictly forward, and reading ends only past the last object
            prop_assert_eq!(&read, &model);
            prop_assert!(cursors.windows(2).all(|pair| pair[0] < pair[1]));
            let past_max = read.last() == Some(&u64::MAX);
            prop_assert_eq!(cursor.object_id().is_none(), past_max);
        }

        #[test]
        fn cursors_order_as_the_object_ids_they_stand_before(
            a in id_near_bounds(),
            b in id_near_bounds(),
        ) {
            // Act / Assert: the cursor after `a` stands before exactly the ids above `a`
            prop_assert_eq!(ObjectCursor::At(a).cmp(&ObjectCursor::At(b)), a.cmp(&b));
            prop_assert_eq!(ObjectCursor::At(b) < ObjectCursor::after(a), b <= a);
        }
    }
}
