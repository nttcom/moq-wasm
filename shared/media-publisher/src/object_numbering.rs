use anyhow::{Result, ensure};
use moqt::Location;

use crate::manager::GroupBoundary;

/// Assigns every object of a track its location before any subscriber exists,
/// so the FETCH cache and the live stream agree on group and object ids. The
/// first group may start at any chosen id; later chosen ids must move forward.
pub(crate) struct ObjectNumbering {
    next_group_id: u64,
    open_group: Option<OpenGroup>,
}

struct OpenGroup {
    group_id: u64,
    next_object_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Placement {
    pub(crate) location: Location,
    pub(crate) starts_group: bool,
}

impl ObjectNumbering {
    pub(crate) fn new(first_group_id: u64) -> Self {
        Self {
            next_group_id: first_group_id,
            open_group: None,
        }
    }

    pub(crate) fn next_group_id(&self) -> u64 {
        self.next_group_id
    }

    pub(crate) fn place(&mut self, boundary: GroupBoundary) -> Result<Option<Placement>> {
        match boundary {
            GroupBoundary::Within => Ok(self.next_in_open_group()),
            GroupBoundary::Next => Ok(Some(self.start_group(self.next_group_id))),
            GroupBoundary::At(group_id) => {
                ensure!(
                    self.open_group.is_none() || group_id >= self.next_group_id,
                    "group {group_id} precedes the next group {}",
                    self.next_group_id
                );
                Ok(Some(self.start_group(group_id)))
            }
        }
    }

    fn next_in_open_group(&mut self) -> Option<Placement> {
        let group = self.open_group.as_mut()?;
        let location = Location {
            group_id: group.group_id,
            object_id: group.next_object_id,
        };
        group.next_object_id += 1;
        Some(Placement {
            location,
            starts_group: false,
        })
    }

    fn start_group(&mut self, group_id: u64) -> Placement {
        self.open_group = Some(OpenGroup {
            group_id,
            next_object_id: 1,
        });
        self.next_group_id = group_id + 1;
        Placement {
            location: Location {
                group_id,
                object_id: 0,
            },
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

        // Assert
        assert_eq!(skipped, placed(12, 0, true));
        assert_eq!(numbering.next_group_id(), 13);
        assert!(stale.is_err());
    }
}
