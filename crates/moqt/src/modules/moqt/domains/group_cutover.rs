/// Decides which of a track's two subscriptions each object is taken from
/// while a session migrates: groups before the cutover come from the
/// previous subscription, the cutover group and later ones from the
/// current one. The current subscription's first object fixes the cutover
/// at the first group it delivers from its start that the previous one has
/// not started, so no group is taken twice or without its start. The
/// cutover does not wait for the current subscription to deliver a group
/// first: the publisher sends to both servers at once, so the previous one
/// may win every race.
pub(crate) struct GroupCutover {
    newest: Option<u64>,
    /// The first group the current subscription delivers whole; it joins
    /// earlier groups mid-way.
    current_start: u64,
    state: Cutover,
}

#[derive(Clone, Copy, PartialEq)]
enum Cutover {
    Waiting,
    At(u64),
}

impl GroupCutover {
    /// Waiting for the current subscription, after the previous one handed
    /// out groups up to `newest`.
    pub(crate) fn waiting_after(newest: Option<u64>, current_start: u64) -> Self {
        Self {
            newest,
            current_start,
            state: Cutover::Waiting,
        }
    }

    /// The previous subscription ended. If the current one has delivered
    /// nothing yet, it starts at its first group start, even when its group
    /// IDs are lower.
    pub(crate) fn previous_ended(&mut self) {
        if self.state == Cutover::Waiting {
            self.newest = None;
        }
    }

    /// Lets the previous subscription go while no group of it is arriving,
    /// so it hands out nothing more. A waiting cutover is fixed at the group
    /// after the newest it handed out, since the same publisher numbers the
    /// groups alike on both connections. Returns false, releasing nothing,
    /// while that group lies before the current subscription's start: only
    /// the previous one can deliver the groups in between.
    pub(crate) fn release_previous(&mut self) -> bool {
        if self.state == Cutover::Waiting {
            let next = self.newest.map_or(0, |newest| newest + 1);
            if self.newest.is_some() && next < self.current_start {
                return false;
            }
            self.state = Cutover::At(next);
        }
        true
    }

    pub(crate) fn is_cut_over(&self) -> bool {
        matches!(self.state, Cutover::At(_))
    }

    pub(crate) fn take_previous(&mut self, group_id: u64) -> bool {
        let take = match self.state {
            Cutover::At(cutover) => group_id < cutover,
            Cutover::Waiting => true,
        };
        if take {
            self.note(group_id);
        }
        take
    }

    pub(crate) fn take_current(&mut self, group_id: u64, object_id: u64) -> bool {
        let take = match self.state {
            Cutover::At(cutover) => group_id >= cutover,
            Cutover::Waiting => {
                let cutover = match self.newest {
                    Some(newest) if group_id <= newest => newest + 1,
                    _ if object_id == 0 && group_id >= self.current_start => group_id,
                    _ => group_id + 1,
                };
                self.state = Cutover::At(cutover);
                group_id >= cutover
            }
        };
        if take {
            self.note(group_id);
        }
        take
    }

    fn note(&mut self, group_id: u64) {
        self.newest = Some(self.newest.map_or(group_id, |newest| newest.max(group_id)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrating_after(groups: &[u64]) -> GroupCutover {
        GroupCutover::waiting_after(groups.iter().copied().max(), 0)
    }

    #[test]
    fn the_first_newer_group_start_on_the_new_session_is_the_cutover() {
        // Arrange
        let mut cutover = migrating_after(&[1]);

        // Act
        let previous_tail = cutover.take_previous(1);
        let current_start = cutover.take_current(2, 0);
        let late_previous_tail = cutover.take_previous(1);
        let previous_cutover_group = cutover.take_previous(2);
        let current_rest = cutover.take_current(2, 1);

        // Assert
        assert!(previous_tail);
        assert!(current_start);
        assert!(cutover.is_cut_over());
        assert!(late_previous_tail);
        assert!(!previous_cutover_group);
        assert!(current_rest);
    }

    #[test]
    fn groups_the_previous_session_already_delivered_stay_with_it() {
        // Arrange
        let mut cutover = migrating_after(&[1]);
        assert!(cutover.take_previous(2));

        // Act
        let current_old_group = cutover.take_current(2, 0);
        let previous_rest = cutover.take_previous(2);
        let current_next_group = cutover.take_current(3, 0);

        // Assert
        assert!(!current_old_group);
        assert!(previous_rest);
        assert!(current_next_group);
    }

    #[test]
    fn a_group_joined_after_its_start_is_left_to_the_previous_session() {
        // Arrange
        let mut cutover = migrating_after(&[1]);

        // Act
        let current_mid_group = cutover.take_current(2, 3);
        let previous_group = cutover.take_previous(2);
        let current_next_group = cutover.take_current(3, 0);

        // Assert
        assert!(!current_mid_group);
        assert!(cutover.is_cut_over());
        assert!(previous_group);
        assert!(current_next_group);
    }

    #[test]
    fn a_group_the_current_session_joined_mid_way_is_left_to_the_previous_session() {
        // Arrange: the current subscription delivers whole groups from group 3
        let mut cutover = GroupCutover::waiting_after(Some(1), 3);

        // Act
        let current_joined_group = cutover.take_current(2, 0);
        let previous_group = cutover.take_previous(2);
        let current_next_group = cutover.take_current(3, 0);

        // Assert
        assert!(!current_joined_group);
        assert!(previous_group);
        assert!(current_next_group);
    }

    #[test]
    fn a_previous_session_that_always_delivers_first_still_hands_over() {
        // Arrange
        let mut cutover = migrating_after(&[1]);
        assert!(cutover.take_previous(2));

        // Act
        let current_same_group = cutover.take_current(2, 0);
        let previous_next_group = cutover.take_previous(3);
        let current_next_group = cutover.take_current(3, 0);

        // Assert
        assert!(!current_same_group);
        assert!(!previous_next_group);
        assert!(current_next_group);
    }

    #[test]
    fn a_released_previous_session_takes_no_later_group_and_the_current_one_resumes_after_it() {
        // Arrange
        let mut cutover = migrating_after(&[1]);
        assert!(cutover.take_previous(2));

        // Act
        cutover.release_previous();
        let previous_next_group = cutover.take_previous(3);
        let current_delivered_group = cutover.take_current(2, 0);
        let current_next_group = cutover.take_current(3, 0);

        // Assert
        assert!(cutover.is_cut_over());
        assert!(!previous_next_group);
        assert!(!current_delivered_group);
        assert!(current_next_group);
    }

    #[test]
    fn the_previous_session_is_kept_for_the_groups_before_the_current_start() {
        // Arrange: the current subscription starts at group 3
        let mut cutover = GroupCutover::waiting_after(Some(1), 3);

        // Act
        let released_owing = cutover.release_previous();
        let previous_owed_group = cutover.take_previous(2);
        let released_after_it = cutover.release_previous();
        let current_start = cutover.take_current(3, 0);

        // Assert
        assert!(!released_owing);
        assert!(previous_owed_group);
        assert!(released_after_it);
        assert!(current_start);
    }

    #[test]
    fn releasing_after_the_cutover_keeps_it() {
        // Arrange
        let mut cutover = migrating_after(&[1]);
        assert!(cutover.take_current(3, 0));

        // Act
        cutover.release_previous();
        let previous_tail = cutover.take_previous(2);
        let current_group = cutover.take_current(3, 1);

        // Assert
        assert!(previous_tail);
        assert!(current_group);
    }

    #[test]
    fn without_the_previous_session_the_next_group_start_is_the_cutover() {
        // Arrange: the publisher numbered the groups on the new session lower
        let mut cutover = migrating_after(&[10]);

        // Act
        cutover.previous_ended();
        let current_mid_group = cutover.take_current(3, 1);
        let current_start = cutover.take_current(4, 0);

        // Assert
        assert!(!current_mid_group);
        assert!(current_start);
    }

    mod properties {
        use std::collections::{BTreeSet, HashMap};

        use proptest::{collection::vec, option, prelude::*};

        use super::super::GroupCutover;

        /// The groups both subscriptions carry, numbered alike by the one
        /// publisher: few, so the subscriptions often meet in a group.
        const GROUPS: u64 = 10;

        #[derive(Debug, Clone)]
        enum Event {
            /// The previous subscription delivers an object of a group.
            Previous {
                group_id: u64,
            },
            /// The current subscription delivers an object; a subgroup
            /// stream arrives as its group's object 0.
            Current {
                group_id: u64,
                object_id: u64,
            },
            ReleasePrevious,
            PreviousEnded,
        }

        /// The previous subscription handed out the groups up to `newest`.
        /// The current one joins `joined` mid-way when `joins_mid_way`, and
        /// delivers every later group whole, from `current_start`.
        #[derive(Debug, Clone)]
        struct Migration {
            newest: Option<u64>,
            joined: u64,
            joins_mid_way: bool,
            events: Vec<Event>,
        }

        impl Migration {
            fn current_start(&self) -> u64 {
                self.joined + u64::from(self.joins_mid_way)
            }
        }

        fn migration() -> impl Strategy<Value = Migration> {
            (option::of(0..6u64), 0..6u64, any::<bool>()).prop_flat_map(
                |(newest, joined, joins_mid_way)| {
                    let event = prop_oneof![
                        4 => (0..GROUPS).prop_map(|group_id| Event::Previous { group_id }),
                        4 => (joined..joined + 4, 0..3u64).prop_map(|(group_id, object_id)| {
                            Event::Current {
                                group_id,
                                object_id,
                            }
                        }),
                        1 => Just(Event::ReleasePrevious),
                        1 => Just(Event::PreviousEnded),
                    ];
                    vec(event, 0..24).prop_map(move |events| Migration {
                        newest,
                        joined,
                        joins_mid_way,
                        events,
                    })
                },
            )
        }

        proptest! {
            #[test]
            fn each_group_comes_whole_from_one_subscription(migration in migration()) {
                // Arrange
                let current_start = migration.current_start();
                let mut cutover = GroupCutover::waiting_after(migration.newest, current_start);
                // The groups up to `newest` count as handed out already.
                let mut previous_taken: BTreeSet<u64> =
                    (0..migration.newest.map_or(0, |newest| newest + 1)).collect();
                let mut current_taken = BTreeSet::new();
                // (from the previous subscription, group ID) → taken
                let mut decisions = HashMap::new();
                let mut previous_ended = false;
                // Released while the previous subscription had handed out
                // nothing, which fixes the cutover at group 0.
                let mut released_empty = false;

                for event in migration.events {
                    match event {
                        Event::Previous { group_id } => {
                            if previous_ended {
                                continue;
                            }
                            // Act
                            let taken = cutover.take_previous(group_id);

                            // Assert: a subscription either takes every
                            // object of a group or none
                            prop_assert_eq!(*decisions.entry((true, group_id)).or_insert(taken), taken);
                            if taken {
                                prop_assert!(!current_taken.contains(&group_id), "group {} taken from both", group_id);
                                previous_taken.insert(group_id);
                            }
                        }
                        Event::Current { group_id, object_id } => {
                            // Act
                            let taken = cutover.take_current(group_id, object_id);

                            // Assert: a group the current subscription joined
                            // mid-way is taken only once the previous one can
                            // no longer deliver it
                            prop_assert_eq!(*decisions.entry((false, group_id)).or_insert(taken), taken);
                            if taken {
                                prop_assert!(
                                    !previous_taken.contains(&group_id),
                                    "group {} taken from both", group_id
                                );
                                prop_assert!(
                                    group_id >= current_start || previous_ended || released_empty,
                                    "group {} taken without its start", group_id
                                );
                                current_taken.insert(group_id);
                            }
                        }
                        Event::ReleasePrevious => {
                            let waiting = !cutover.is_cut_over();

                            // Act
                            let released = cutover.release_previous();

                            // Assert
                            prop_assert_eq!(cutover.is_cut_over(), released);
                            released_empty |= waiting && released && previous_taken.is_empty();
                        }
                        Event::PreviousEnded => {
                            if !cutover.is_cut_over() {
                                // It forgets what the previous subscription handed
                                // out, as the publisher may number the groups on the
                                // current one lower.
                                previous_taken.clear();
                            }

                            // Act
                            cutover.previous_ended();
                            previous_ended = true;
                        }
                    }
                }
            }
        }
    }
}
