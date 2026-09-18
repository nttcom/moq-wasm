struct NewestGroup {
    group_id: u64,
    sequence: u64,
}

/// Counts the groups a subscription has opened so far, in group id order.
pub(super) struct GroupSequence {
    newest: Option<NewestGroup>,
}

impl GroupSequence {
    pub(super) fn new() -> Self {
        Self { newest: None }
    }

    pub(super) fn sequence_of(&mut self, group_id: u64) -> u64 {
        match &mut self.newest {
            None => {
                self.newest = Some(NewestGroup {
                    group_id,
                    sequence: 0,
                });
                0
            }
            Some(newest) if group_id == newest.group_id => newest.sequence,
            Some(newest) if group_id > newest.group_id => {
                newest.group_id = group_id;
                newest.sequence += 1;
                newest.sequence
            }
            Some(newest) => newest.sequence.saturating_sub(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_group_starts_the_sequence_at_zero() {
        // Arrange
        let mut sequence = GroupSequence::new();
        // Act / Assert
        assert_eq!(sequence.sequence_of(1_790_000_000_000_000), 0);
    }

    #[test]
    fn subgroups_of_the_same_group_share_the_sequence() {
        // Arrange
        let mut sequence = GroupSequence::new();
        sequence.sequence_of(10);
        // Act / Assert
        assert_eq!(sequence.sequence_of(10), 0);
    }

    #[test]
    fn each_newer_group_increments_the_sequence_regardless_of_id_gap() {
        // Arrange
        let mut sequence = GroupSequence::new();
        sequence.sequence_of(10);
        // Act
        let next = sequence.sequence_of(11);
        let far = sequence.sequence_of(2_000_000);
        // Assert
        assert_eq!((next, far), (1, 2));
    }

    #[test]
    fn older_group_ranks_just_before_the_newest_without_replacing_it() {
        // Arrange
        let mut sequence = GroupSequence::new();
        sequence.sequence_of(10);
        sequence.sequence_of(11);
        sequence.sequence_of(12);
        // Act
        let older = sequence.sequence_of(9);
        let newest_again = sequence.sequence_of(12);
        // Assert
        assert_eq!((older, newest_again), (1, 2));
    }
}
