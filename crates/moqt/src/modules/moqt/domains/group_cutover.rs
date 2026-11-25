use tokio::sync::oneshot;

/// Decides which of a track's two subscriptions each object is taken from
/// while a session migrates: groups before the cutover come from the
/// previous subscription, the cutover group and later ones from the
/// current one. The current subscription's first object fixes the cutover
/// at the first group it delivers whole that the previous one has not
/// started, so no group is taken twice or without its start, whichever of
/// the current one's groups arrives first. The cutover does not wait for
/// the current subscription to deliver a group first: the publisher sends
/// to both servers at once, so the previous one may win every race.
///
/// What the previous subscription handed out is kept after its connection
/// closes, so a lagging relay's groups are not taken twice. This relies on
/// the publisher never reusing a location: one that numbers the groups on
/// the current connection lower has nothing taken from it until its group
/// IDs pass the newest the previous one handed out.
pub(crate) struct GroupCutover {
    /// The first group the current subscription delivers whole; it joins
    /// earlier groups mid-way.
    current_start: u64,
    state: Cutover,
}

enum Cutover {
    /// No cutover group yet; dropping `_cut_over` signals that it is fixed.
    Waiting {
        /// The newest group the previous subscription handed out.
        newest: Option<u64>,
        _cut_over: oneshot::Sender<()>,
    },
    /// Groups before it come from the previous subscription, the rest from
    /// the current one.
    At(u64),
}

impl GroupCutover {
    /// Waiting for the current subscription, after the previous one handed
    /// out groups up to `newest`. `cut_over` is dropped once the cutover
    /// group is fixed.
    pub(crate) fn waiting_after(
        newest: Option<u64>,
        current_start: u64,
        cut_over: oneshot::Sender<()>,
    ) -> Self {
        Self {
            current_start,
            state: Cutover::Waiting {
                newest,
                _cut_over: cut_over,
            },
        }
    }

    /// Lets the previous subscription go while no group of it is arriving,
    /// so it hands out nothing more. A waiting cutover is fixed at the group
    /// after the newest it handed out, since the same publisher numbers the
    /// groups alike on both connections, and not before the current
    /// subscription's first whole group. Returns false, releasing nothing,
    /// while the group after the newest lies before that start: only the
    /// previous one can deliver the groups in between.
    pub(crate) fn release_previous(&mut self) -> bool {
        let Cutover::Waiting { newest, .. } = self.state else {
            return true;
        };
        if newest.is_some_and(|newest| newest + 1 < self.current_start) {
            return false;
        }
        self.state = Cutover::At(self.cutover_after(newest));
        true
    }

    pub(crate) fn take_previous(&mut self, group_id: u64) -> bool {
        match &mut self.state {
            Cutover::At(cutover) => group_id < *cutover,
            Cutover::Waiting { newest, .. } => {
                *newest = Some(newest.map_or(group_id, |newest| newest.max(group_id)));
                true
            }
        }
    }

    pub(crate) fn take_current(&mut self, group_id: u64) -> bool {
        let cutover = match &self.state {
            Cutover::At(cutover) => *cutover,
            Cutover::Waiting { newest, .. } => {
                let cutover = self.cutover_after(*newest);
                self.state = Cutover::At(cutover);
                cutover
            }
        };
        group_id >= cutover
    }

    /// The first group the current subscription delivers whole after the
    /// previous one handed out groups up to `newest`.
    fn cutover_after(&self, newest: Option<u64>) -> u64 {
        newest
            .map_or(0, |newest| newest + 1)
            .max(self.current_start)
    }

    #[cfg(test)]
    fn cutover(&self) -> Option<u64> {
        match self.state {
            Cutover::Waiting { .. } => None,
            Cutover::At(cutover) => Some(cutover),
        }
    }

    #[cfg(test)]
    fn is_cut_over(&self) -> bool {
        self.cutover().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiting_after(newest: Option<u64>, current_start: u64) -> GroupCutover {
        GroupCutover::waiting_after(newest, current_start, oneshot::channel().0)
    }

    fn migrating_after(groups: &[u64]) -> GroupCutover {
        waiting_after(groups.iter().copied().max(), 0)
    }

    #[test]
    fn the_first_newer_group_start_on_the_new_session_is_the_cutover() {
        // Arrange
        let mut cutover = migrating_after(&[1]);

        // Act
        let previous_tail = cutover.take_previous(1);
        let current_start = cutover.take_current(2);
        let late_previous_tail = cutover.take_previous(1);
        let previous_cutover_group = cutover.take_previous(2);
        let current_rest = cutover.take_current(2);

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
        let current_old_group = cutover.take_current(2);
        let previous_rest = cutover.take_previous(2);
        let current_next_group = cutover.take_current(3);

        // Assert
        assert!(!current_old_group);
        assert!(previous_rest);
        assert!(current_next_group);
    }

    #[test]
    fn a_group_joined_after_its_start_is_left_to_the_previous_session() {
        // Arrange: the current subscription joins group 2 mid-way
        let mut cutover = waiting_after(Some(1), 3);

        // Act
        let current_mid_group = cutover.take_current(2);
        let previous_group = cutover.take_previous(2);
        let current_next_group = cutover.take_current(3);

        // Assert
        assert!(!current_mid_group);
        assert!(cutover.is_cut_over());
        assert!(previous_group);
        assert!(current_next_group);
    }

    #[test]
    fn a_group_the_current_session_joined_mid_way_is_left_to_the_previous_session() {
        // Arrange: the current subscription delivers whole groups from group 3
        let mut cutover = waiting_after(Some(1), 3);

        // Act
        let current_joined_group = cutover.take_current(2);
        let previous_group = cutover.take_previous(2);
        let current_next_group = cutover.take_current(3);

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
        let current_same_group = cutover.take_current(2);
        let previous_next_group = cutover.take_previous(3);
        let current_next_group = cutover.take_current(3);

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
        let current_delivered_group = cutover.take_current(2);
        let current_next_group = cutover.take_current(3);

        // Assert
        assert!(cutover.is_cut_over());
        assert!(!previous_next_group);
        assert!(!current_delivered_group);
        assert!(current_next_group);
    }

    #[test]
    fn a_group_the_current_session_delivers_whole_after_a_later_one_is_kept() {
        // Arrange: the current subscription delivers whole groups from group 2
        let mut cutover = waiting_after(Some(1), 2);

        // Act: group 3's stream overtakes group 2's
        let current_later_group = cutover.take_current(3);
        let current_start = cutover.take_current(2);

        // Assert
        assert!(current_later_group);
        assert!(current_start);
    }

    #[test]
    fn the_previous_session_is_kept_for_the_groups_before_the_current_start() {
        // Arrange: the current subscription starts at group 3
        let mut cutover = waiting_after(Some(1), 3);

        // Act
        let released_owing = cutover.release_previous();
        let previous_owed_group = cutover.take_previous(2);
        let released_after_it = cutover.release_previous();
        let current_start = cutover.take_current(3);

        // Assert
        assert!(!released_owing);
        assert!(previous_owed_group);
        assert!(released_after_it);
        assert!(current_start);
    }

    #[test]
    fn a_current_subscription_without_a_whole_group_keeps_the_previous_one() {
        // Arrange: the migration passes u64::MAX when the current one
        // delivers no whole group
        let mut cutover = waiting_after(Some(1), u64::MAX);

        // Act
        let released = cutover.release_previous();
        let previous_next_group = cutover.take_previous(2);

        // Assert
        assert!(!released);
        assert!(previous_next_group);
        assert!(!cutover.is_cut_over());
    }

    #[test]
    fn releasing_after_the_cutover_keeps_it() {
        // Arrange: the current subscription delivers whole groups from group 3
        let mut cutover = waiting_after(Some(1), 3);
        assert!(cutover.take_current(3));

        // Act
        cutover.release_previous();
        let previous_tail = cutover.take_previous(2);
        let current_group = cutover.take_current(3);

        // Assert
        assert!(previous_tail);
        assert!(current_group);
    }

    #[test]
    fn groups_a_lagging_relay_delivers_again_are_dropped() {
        // Arrange: the previous session handed out up to group 5 and its
        // connection closed; the current relay lags and delivers whole
        // groups from group 3
        let mut cutover = waiting_after(Some(5), 3);

        // Act
        let current_delivered_group = cutover.take_current(4);
        let current_next_group = cutover.take_current(6);

        // Assert
        assert!(!current_delivered_group);
        assert!(current_next_group);
    }

    #[test]
    fn groups_numbered_lower_on_the_new_session_are_taken_once_past_the_newest_handed_out() {
        // Arrange: the publisher numbered the groups on the new session lower
        let mut cutover = waiting_after(Some(10), 4);

        // Act
        let current_start = cutover.take_current(4);
        let current_past_newest = cutover.take_current(11);

        // Assert
        assert!(!current_start);
        assert!(current_past_newest);
    }

    mod properties {
        use std::collections::{BTreeSet, HashMap};

        use proptest::{collection::vec, option, prelude::*};
        use tokio::sync::oneshot::{self, error::TryRecvError};

        use super::{super::GroupCutover, waiting_after};

        /// The groups both subscriptions carry, numbered alike by the one
        /// publisher: few, so the subscriptions often meet in a group.
        const GROUPS: u64 = 10;

        #[derive(Debug, Clone)]
        enum Event {
            /// The previous subscription delivers an object of a group.
            Previous {
                group_id: u64,
            },
            /// The current subscription delivers an object of a group.
            Current {
                group_id: u64,
            },
            ReleasePrevious,
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
                        4 => (joined..joined + 4).prop_map(|group_id| Event::Current { group_id }),
                        1 => Just(Event::ReleasePrevious),
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
                let mut cutover = waiting_after(migration.newest, current_start);
                // The groups up to `newest` count as handed out already.
                let mut previous_taken: BTreeSet<u64> =
                    (0..migration.newest.map_or(0, |newest| newest + 1)).collect();
                let mut current_taken = BTreeSet::new();
                // Group ID → taken from the previous subscription
                let mut decisions = HashMap::new();

                for event in migration.events {
                    match event {
                        Event::Previous { group_id } => {
                            // Act
                            let taken = cutover.take_previous(group_id);

                            // Assert: a subscription either takes every
                            // object of a group or none
                            prop_assert_eq!(*decisions.entry(group_id).or_insert(taken), taken);
                            if taken {
                                prop_assert!(!current_taken.contains(&group_id), "group {} taken from both", group_id);
                                previous_taken.insert(group_id);
                            }
                        }
                        Event::Current { group_id } => {
                            // Act
                            let taken = cutover.take_current(group_id);

                            // Assert: the current subscription takes exactly
                            // the groups it delivers whole that the previous
                            // one has not overtaken, in whatever order they
                            // arrive
                            let delivered_whole = group_id >= current_start;
                            let overtaken = previous_taken.last().is_some_and(|newest| group_id <= *newest);
                            prop_assert_eq!(taken, delivered_whole && !overtaken, "group {}", group_id);
                            if taken {
                                current_taken.insert(group_id);
                            }
                        }
                        Event::ReleasePrevious => {
                            // Act
                            let released = cutover.release_previous();

                            // Assert
                            prop_assert_eq!(cutover.is_cut_over(), released);
                        }
                    }
                }
            }

            #[test]
            fn a_fixed_cutover_separates_the_subscriptions_and_is_signalled(migration in migration()) {
                // Arrange
                let (cut_over, mut cut_over_receiver) = oneshot::channel();
                let mut cutover = GroupCutover::waiting_after(
                    migration.newest,
                    migration.current_start(),
                    cut_over,
                );
                // The groups up to `newest` count as handed out already.
                let mut previous_taken: BTreeSet<u64> =
                    (0..migration.newest.map_or(0, |newest| newest + 1)).collect();
                let mut current_taken = BTreeSet::new();

                for event in migration.events {
                    // Act
                    match event {
                        Event::Previous { group_id } => {
                            if cutover.take_previous(group_id) {
                                previous_taken.insert(group_id);
                            }
                        }
                        Event::Current { group_id } => {
                            if cutover.take_current(group_id) {
                                current_taken.insert(group_id);
                            }
                        }
                        Event::ReleasePrevious => {
                            cutover.release_previous();
                        }
                    }

                    // Assert: the cutover is signalled exactly when it is
                    // fixed, and then lies above every group taken from the
                    // previous subscription and at or below every group taken
                    // from the current one
                    let signalled = matches!(cut_over_receiver.try_recv(), Err(TryRecvError::Closed));
                    prop_assert_eq!(signalled, cutover.is_cut_over());
                    if let Some(cut) = cutover.cutover() {
                        prop_assert!(
                            previous_taken.iter().all(|group_id| *group_id < cut),
                            "previous groups {:?} not before cutover {}", previous_taken, cut
                        );
                        prop_assert!(
                            current_taken.iter().all(|group_id| *group_id >= cut),
                            "current groups {:?} not from cutover {}", current_taken, cut
                        );
                    }
                }
            }
        }
    }
}
