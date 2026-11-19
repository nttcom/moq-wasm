use std::{collections::BTreeMap, ops::Bound};

use moqt::object::GroupOrder;
use tokio::sync::watch;

/// Group id to its number of running send tasks, one per subgroup.
type Running = BTreeMap<u64, usize>;

/// Ranks a subscription's running groups by its Group Order, for
/// `StreamPriority::group_position`, so the next group of every subscription
/// ties with the others' (draft-14 §7.2 leaves their order to the
/// implementation and expects every subscription to send some data).
pub(super) struct GroupPositions {
    running_sender: watch::Sender<Running>,
    group_order: GroupOrder,
}

impl GroupPositions {
    pub(super) fn new(group_order: GroupOrder) -> Self {
        Self {
            running_sender: watch::Sender::new(Running::new()),
            group_order,
        }
    }

    /// Counts `group_id` as running until the returned position is dropped.
    pub(super) fn enter(&self, group_id: u64) -> GroupPosition {
        self.running_sender
            .send_modify(|running| *running.entry(group_id).or_default() += 1);
        GroupPosition {
            running_sender: self.running_sender.clone(),
            running_receiver: self.running_sender.subscribe(),
            group_id,
            group_order: self.group_order,
        }
    }
}

pub(super) struct GroupPosition {
    running_sender: watch::Sender<Running>,
    running_receiver: watch::Receiver<Running>,
    group_id: u64,
    group_order: GroupOrder,
}

impl GroupPosition {
    pub(super) fn current(&self) -> u64 {
        position_of(
            &self.running_receiver.borrow(),
            self.group_id,
            self.group_order,
        )
    }

    pub(super) async fn moved_from(&mut self, position: u64) {
        let (group_id, group_order) = (self.group_id, self.group_order);
        let at = |running: &Running| position_of(running, group_id, group_order);
        // The sender lives in `self`, so the channel never closes.
        let _ = self
            .running_receiver
            .wait_for(|running| at(running) != position)
            .await;
    }
}

impl Drop for GroupPosition {
    fn drop(&mut self) {
        let group_id = self.group_id;
        self.running_sender.send_modify(|running| {
            if let Some(tasks) = running.get_mut(&group_id) {
                *tasks -= 1;
                if *tasks == 0 {
                    running.remove(&group_id);
                }
            }
        });
    }
}

fn position_of(running: &Running, group_id: u64, group_order: GroupOrder) -> u64 {
    let ahead = match group_order {
        GroupOrder::Descending => running
            .range((Bound::Excluded(group_id), Bound::Unbounded))
            .count(),
        GroupOrder::Ascending | GroupOrder::Publisher => running.range(..group_id).count(),
    };
    ahead as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descending_puts_the_newest_running_group_first() {
        // Arrange
        let positions = GroupPositions::new(GroupOrder::Descending);
        let older = positions.enter(101);
        // Act
        let newer = positions.enter(102);
        // Assert
        assert_eq!((newer.current(), older.current()), (0, 1));
    }

    #[test]
    fn ascending_puts_the_oldest_running_group_first() {
        // Arrange
        let positions = GroupPositions::new(GroupOrder::Ascending);
        let older = positions.enter(101);
        // Act
        let newer = positions.enter(102);
        // Assert
        assert_eq!((older.current(), newer.current()), (0, 1));
    }

    #[test]
    fn a_finished_group_moves_the_next_one_up() {
        // Arrange
        let positions = GroupPositions::new(GroupOrder::Ascending);
        let first = positions.enter(101);
        let second = positions.enter(102);
        // Act
        drop(first);
        // Assert
        assert_eq!(second.current(), 0);
    }

    #[test]
    fn subgroups_of_one_group_share_its_position_until_the_last_ends() {
        // Arrange
        let positions = GroupPositions::new(GroupOrder::Ascending);
        let subgroup_0 = positions.enter(101);
        let subgroup_1 = positions.enter(101);
        let next_group = positions.enter(102);
        // Act
        drop(subgroup_0);
        // Assert
        assert_eq!((subgroup_1.current(), next_group.current()), (0, 1));
    }

    #[tokio::test]
    async fn a_newer_group_moves_a_descending_group_back() {
        // Arrange
        let positions = GroupPositions::new(GroupOrder::Descending);
        let mut older = positions.enter(101);
        let at = older.current();
        // Act
        let _newer = positions.enter(102);
        // Assert
        tokio::time::timeout(std::time::Duration::from_secs(1), older.moved_from(at))
            .await
            .expect("the older group should move");
        assert_eq!(older.current(), 1);
    }
}
