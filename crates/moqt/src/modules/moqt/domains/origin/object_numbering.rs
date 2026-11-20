use anyhow::{Context, Result, ensure};

use crate::object::Location;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBoundary {
    /// Dropped when no group is open: a group must not start on it.
    Within,
    Next,
    At(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub location: Location,
    pub starts_group: bool,
}

pub(super) struct ObjectNumbering {
    /// The group `GroupBoundary::Next` starts while no group is open.
    first_group_id: u64,
    largest: Option<Location>,
}

impl ObjectNumbering {
    pub(super) fn new(first_group_id: u64) -> Self {
        Self {
            first_group_id,
            largest: None,
        }
    }

    pub(super) fn largest(&self) -> Option<Location> {
        self.largest
    }

    /// Fails for a group that does not follow the open one, and for a group
    /// or object after one numbered u64::MAX.
    pub(super) fn place(&mut self, boundary: GroupBoundary) -> Result<Option<Placement>> {
        match boundary {
            GroupBoundary::Within => self.next_in_open_group(),
            GroupBoundary::Next => {
                let group_id = match self.largest {
                    None => self.first_group_id,
                    Some(largest) => largest
                        .group_id
                        .checked_add(1)
                        .context("no group follows group u64::MAX")?,
                };
                Ok(Some(self.start_group(group_id)))
            }
            GroupBoundary::At(group_id) => {
                if let Some(largest) = self.largest {
                    ensure!(
                        group_id > largest.group_id,
                        "group {group_id} does not follow the open group {}",
                        largest.group_id
                    );
                }
                Ok(Some(self.start_group(group_id)))
            }
        }
    }

    fn next_in_open_group(&mut self) -> Result<Option<Placement>> {
        let Some(largest) = &mut self.largest else {
            return Ok(None);
        };
        largest.object_id = largest
            .object_id
            .checked_add(1)
            .context("no object follows object u64::MAX")?;
        Ok(Some(Placement {
            location: *largest,
            starts_group: false,
        }))
    }

    fn start_group(&mut self, group_id: u64) -> Placement {
        let location = Location {
            group_id,
            object_id: 0,
        };
        self.largest = Some(location);
        Placement {
            location,
            starts_group: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(group_id: u64, object_id: u64, starts_group: bool) -> Option<Placement> {
        Some(Placement {
            location: Location {
                group_id,
                object_id,
            },
            starts_group,
        })
    }

    #[test]
    fn drops_within_objects_until_a_group_is_open_and_numbers_objects_per_group() {
        // Arrange
        let mut numbering = ObjectNumbering::new(10);

        // Act
        let dropped = numbering.place(GroupBoundary::Within).unwrap();
        let first = numbering.place(GroupBoundary::Next).unwrap();
        let second = numbering.place(GroupBoundary::Within).unwrap();
        let next_group = numbering.place(GroupBoundary::Next).unwrap();

        // Assert
        assert_eq!(dropped, None);
        assert_eq!(first, placed(10, 0, true));
        assert_eq!(second, placed(10, 1, false));
        assert_eq!(next_group, placed(11, 0, true));
    }

    #[test]
    fn the_first_group_may_start_before_the_seed() {
        // Arrange
        let mut numbering = ObjectNumbering::new(500);

        // Act
        let first = numbering.place(GroupBoundary::At(8)).unwrap();
        let next = numbering.place(GroupBoundary::Next).unwrap();

        // Assert
        assert_eq!(first, placed(8, 0, true));
        assert_eq!(next, placed(9, 0, true));
    }

    #[test]
    fn chosen_group_ids_move_the_numbering_forward_and_may_not_go_back() {
        // Arrange
        let mut numbering = ObjectNumbering::new(5);
        numbering.place(GroupBoundary::At(8)).unwrap();

        // Act
        let skipped = numbering.place(GroupBoundary::At(12)).unwrap();
        let stale = numbering.place(GroupBoundary::At(12));
        let next = numbering.place(GroupBoundary::Next).unwrap();

        // Assert
        assert_eq!(skipped, placed(12, 0, true));
        assert!(stale.is_err());
        assert_eq!(next, placed(13, 0, true));
    }

    #[test]
    fn the_largest_group_id_is_written_and_no_group_follows_it() {
        // Arrange
        let mut numbering = ObjectNumbering::new(0);

        // Act
        let last = numbering.place(GroupBoundary::At(u64::MAX)).unwrap();
        let within = numbering.place(GroupBoundary::Within).unwrap();
        let next = numbering.place(GroupBoundary::Next);

        // Assert: the failed `Next` leaves group u64::MAX open
        assert_eq!(last, placed(u64::MAX, 0, true));
        assert_eq!(within, placed(u64::MAX, 1, false));
        assert!(next.is_err());
        assert_eq!(
            numbering.largest(),
            Some(Location {
                group_id: u64::MAX,
                object_id: 1,
            })
        );
    }

    mod properties {
        use proptest::{collection::vec, prelude::*};

        use super::super::*;

        /// Group IDs clustered at 0, at draft-14's varint limit (2^62) and at
        /// u64::MAX, so a sequence repeats, steps back and runs out of IDs.
        fn group_id() -> impl Strategy<Value = u64> {
            prop_oneof![
                0..4u64,
                (1u64 << 62) - 2..=(1 << 62) + 1,
                u64::MAX - 3..=u64::MAX,
            ]
        }

        fn boundary() -> impl Strategy<Value = GroupBoundary> {
            prop_oneof![
                Just(GroupBoundary::Within),
                Just(GroupBoundary::Next),
                group_id().prop_map(GroupBoundary::At),
            ]
        }

        proptest! {
            #[test]
            fn each_placement_follows_the_largest_object_and_ids_never_wrap(
                first_group_id in group_id(),
                boundaries in vec(boundary(), 1..32),
            ) {
                // Arrange
                let mut numbering = ObjectNumbering::new(first_group_id);

                for boundary in boundaries {
                    let before = numbering.largest();

                    // Act
                    let placed = numbering.place(boundary);

                    // Assert: a placement is the least Location its boundary allows after `before`; a failure changes nothing
                    let expected = match (boundary, before) {
                        (GroupBoundary::Within, None) => Some(None),
                        (GroupBoundary::Within, Some(largest)) => Some(Some((largest.group_id, largest.object_id + 1))),
                        (GroupBoundary::Next, None) => Some(Some((first_group_id, 0))),
                        (GroupBoundary::Next, Some(largest)) => largest.group_id.checked_add(1).map(|group_id| Some((group_id, 0))),
                        (GroupBoundary::At(group_id), largest) => largest
                            .is_none_or(|largest| group_id > largest.group_id)
                            .then_some(Some((group_id, 0))),
                    };
                    match (placed, expected) {
                        (Ok(placed), Some(expected)) => {
                            let placed = placed.map(|placement| {
                                prop_assert_eq!(placement.starts_group, placement.location.object_id == 0);
                                prop_assert!(before.is_none_or(|largest| placement.location > largest));
                                Ok((placement.location.group_id, placement.location.object_id))
                            }).transpose()?;
                            prop_assert_eq!(placed, expected);
                        }
                        (Err(_), None) => prop_assert_eq!(numbering.largest(), before),
                        (placed, expected) => prop_assert!(false, "{boundary:?} after {before:?}: {placed:?}, expected {expected:?}"),
                    }
                }
            }
        }
    }
}
