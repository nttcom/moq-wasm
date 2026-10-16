/// Decides which of a track's two subscriptions each object is taken from
/// while a session migrates: groups before the cutover come from the
/// previous subscription, the cutover group and later ones from the
/// current one. The current subscription's first object fixes the cutover
/// at the first group it delivers from its start that the previous one has
/// not started, so no group is taken twice or without its start. The
/// cutover does not wait for the current subscription to deliver a group
/// first: the publisher sends to both servers at once, so the previous one
/// may win every race.
#[derive(Default)]
pub(crate) struct GroupCutover {
    newest: Option<u64>,
    /// `None` while the track has one subscription.
    state: Option<Cutover>,
}

#[derive(Clone, Copy, PartialEq)]
enum Cutover {
    Waiting,
    At(u64),
}

impl GroupCutover {
    /// Waiting for the current subscription, after the previous one handed
    /// out groups up to `newest`.
    pub(crate) fn waiting_after(newest: Option<u64>) -> Self {
        Self {
            newest,
            state: Some(Cutover::Waiting),
        }
    }

    /// A subscription on the new session became the current one.
    pub(crate) fn begin(&mut self) {
        self.state = Some(Cutover::Waiting);
    }

    /// The previous subscription ended. If the current one has delivered
    /// nothing yet, it starts at its first group start, even when its group
    /// IDs are lower.
    pub(crate) fn previous_ended(&mut self) {
        if self.state == Some(Cutover::Waiting) {
            self.newest = None;
        }
    }

    /// The previous subscription took over again after the current one ended.
    pub(crate) fn settle(&mut self) {
        self.state = None;
    }

    pub(crate) fn is_cut_over(&self) -> bool {
        matches!(self.state, Some(Cutover::At(_)))
    }

    pub(crate) fn take_previous(&mut self, group_id: u64) -> bool {
        let take = match self.state {
            Some(Cutover::At(cutover)) => group_id < cutover,
            _ => true,
        };
        if take {
            self.note(group_id);
        }
        take
    }

    pub(crate) fn take_current(&mut self, group_id: u64, object_id: u64) -> bool {
        let take = match self.state {
            None => true,
            Some(Cutover::At(cutover)) => group_id >= cutover,
            Some(Cutover::Waiting) => {
                let cutover = match self.newest {
                    Some(newest) if group_id <= newest => newest + 1,
                    _ if object_id == 0 => group_id,
                    _ => group_id + 1,
                };
                self.state = Some(Cutover::At(cutover));
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
        let mut cutover = GroupCutover::default();
        for &group_id in groups {
            assert!(cutover.take_current(group_id, 0));
        }
        cutover.begin();
        cutover
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
}
