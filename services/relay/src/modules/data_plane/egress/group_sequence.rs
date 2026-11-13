use std::sync::OnceLock;

use tokio::time::Instant;

/// `StreamPriority` compares 12 bits of a sequence, so the tick count wraps
/// about every 17 minutes.
const TICK_MILLIS: u128 = 250;

/// Ticks since the first call, on one clock for every subscription of the relay.
pub(super) fn current_tick() -> u64 {
    static STARTED: OnceLock<Instant> = OnceLock::new();
    (STARTED.get_or_init(Instant::now).elapsed().as_millis() / TICK_MILLIS) as u64
}

struct NewestGroup {
    group_id: u64,
    sequence: u64,
}

/// Numbers the groups of one subscription for `StreamPriority::group_sequence`.
/// A newer group always gets a larger number, which is what the Group Order
/// compares within the subscription (draft-14 §7.2 rule 3). The number is the
/// tick the group was first seen at, so subscriptions of equal priorities and
/// ascending order send the group seen first; §7.2 leaves that order to the
/// implementation. A track with more than one group a tick runs ahead of the
/// clock, and its sequence says nothing against other subscriptions.
pub(super) struct GroupSequence {
    newest: Option<NewestGroup>,
}

impl GroupSequence {
    pub(super) fn new() -> Self {
        Self { newest: None }
    }

    /// `tick` is `current_tick()` when the group is scheduled.
    pub(super) fn sequence_of(&mut self, group_id: u64, tick: u64) -> u64 {
        match &mut self.newest {
            None => {
                self.newest = Some(NewestGroup {
                    group_id,
                    sequence: tick,
                });
                tick
            }
            Some(newest) if group_id == newest.group_id => newest.sequence,
            Some(newest) if group_id > newest.group_id => {
                newest.group_id = group_id;
                // Two groups of one tick still differ, or the Group Order could not tell them apart.
                newest.sequence = tick.max(newest.sequence + 1);
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
    fn a_group_takes_the_tick_it_is_first_seen_at_and_keeps_it() {
        // Arrange
        let mut sequence = GroupSequence::new();
        // Act
        let first = sequence.sequence_of(10, 400);
        let again = sequence.sequence_of(10, 404);
        // Assert
        assert_eq!((first, again), (400, 400));
    }

    #[test]
    fn groups_of_two_subscriptions_seen_in_the_same_tick_rank_alike() {
        // Arrange: the second subscription started later and its group IDs differ
        let mut early = GroupSequence::new();
        let mut late = GroupSequence::new();
        early.sequence_of(1, 396);
        // Act
        let from_early = early.sequence_of(2, 400);
        let from_late = late.sequence_of(1_790_000_000_000_000, 400);
        // Assert
        assert_eq!(from_early, from_late);
    }

    #[test]
    fn a_newer_group_ranks_after_the_previous_one_even_within_one_tick() {
        // Arrange
        let mut sequence = GroupSequence::new();
        sequence.sequence_of(10, 400);
        // Act
        let same_tick = sequence.sequence_of(11, 400);
        let later = sequence.sequence_of(2_000_000, 408);
        // Assert
        assert_eq!((same_tick, later), (401, 408));
    }

    #[test]
    fn older_group_ranks_just_before_the_newest_without_replacing_it() {
        // Arrange
        let mut sequence = GroupSequence::new();
        sequence.sequence_of(10, 400);
        sequence.sequence_of(11, 404);
        sequence.sequence_of(12, 408);
        // Act
        let older = sequence.sequence_of(9, 412);
        let newest_again = sequence.sequence_of(12, 412);
        // Assert
        assert_eq!((older, newest_again), (407, 408));
    }
}
