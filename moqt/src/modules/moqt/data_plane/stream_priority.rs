use crate::GroupOrder;

const GROUP_RANK_BITS: u32 = 12;
const GROUP_RANK_MASK: u64 = (1 << GROUP_RANK_BITS) - 1;
const SUBGROUP_BITS: u32 = 3;
const SUBGROUP_MAX: u64 = (1 << SUBGROUP_BITS) - 1;
const GROUP_RANK_SHIFT: u32 = SUBGROUP_BITS;
const PUBLISHER_PRIORITY_SHIFT: u32 = GROUP_RANK_SHIFT + GROUP_RANK_BITS;
const SUBSCRIBER_PRIORITY_SHIFT: u32 = PUBLISHER_PRIORITY_SHIFT + 8;

/// draft-14 §7.2 scheduling inputs of one subgroup stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamPriority {
    pub subscriber_priority: u8,
    pub publisher_priority: u8,
    pub group_order: GroupOrder,
    /// Number of groups the subscription opened before this stream's group.
    pub group_sequence: u64,
    pub subgroup_id: u64,
}

impl StreamPriority {
    /// Transport stream priority: a higher value is transmitted first.
    ///
    /// The four §7.2 rules are packed most significant first into the 31
    /// magnitude bits of an `i32`, so any data stream (negative) stays below
    /// the control stream, which keeps the transport default of 0. The
    /// group rank is `group_sequence` modulo 2^12 (reversed for Descending),
    /// so one group boundary in every 4096 orders as if unprioritized; the
    /// subgroup id saturates at 7, so higher subgroups of one group tie.
    pub fn transport_priority(&self) -> i32 {
        let key = (u64::from(self.subscriber_priority) << SUBSCRIBER_PRIORITY_SHIFT)
            | (u64::from(self.publisher_priority) << PUBLISHER_PRIORITY_SHIFT)
            | (self.group_rank() << GROUP_RANK_SHIFT)
            | self.subgroup_id.min(SUBGROUP_MAX);
        -1 - key as i32
    }

    fn group_rank(&self) -> u64 {
        let sequence = self.group_sequence & GROUP_RANK_MASK;
        match self.group_order {
            GroupOrder::Descending => GROUP_RANK_MASK - sequence,
            GroupOrder::Ascending | GroupOrder::Publisher => sequence,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn priority(
        subscriber_priority: u8,
        publisher_priority: u8,
        group_order: GroupOrder,
        group_sequence: u64,
        subgroup_id: u64,
    ) -> i32 {
        StreamPriority {
            subscriber_priority,
            publisher_priority,
            group_order,
            group_sequence,
            subgroup_id,
        }
        .transport_priority()
    }

    #[test]
    fn lower_subscriber_priority_number_is_sent_first() {
        // Arrange
        let high = priority(0, 128, GroupOrder::Ascending, 0, 0);
        let low = priority(255, 128, GroupOrder::Ascending, 0, 0);
        // Act / Assert
        assert!(high > low);
    }

    #[test]
    fn subscriber_priority_dominates_publisher_priority() {
        // Arrange
        let better_subscriber = priority(10, 255, GroupOrder::Ascending, 4095, 7);
        let better_publisher = priority(11, 0, GroupOrder::Ascending, 0, 0);
        // Act / Assert
        assert!(better_subscriber > better_publisher);
    }

    #[test]
    fn publisher_priority_dominates_group_order() {
        // Arrange
        let better_publisher = priority(128, 0, GroupOrder::Ascending, 4095, 7);
        let earlier_group = priority(128, 1, GroupOrder::Ascending, 0, 0);
        // Act / Assert
        assert!(better_publisher > earlier_group);
    }

    #[test]
    fn ascending_sends_earlier_group_first() {
        // Arrange
        let earlier = priority(128, 128, GroupOrder::Ascending, 0, 0);
        let later = priority(128, 128, GroupOrder::Ascending, 1, 0);
        // Act / Assert
        assert!(earlier > later);
    }

    #[test]
    fn descending_sends_later_group_first() {
        // Arrange
        let earlier = priority(128, 128, GroupOrder::Descending, 0, 0);
        let later = priority(128, 128, GroupOrder::Descending, 1, 0);
        // Act / Assert
        assert!(later > earlier);
    }

    #[test]
    fn publisher_group_order_behaves_as_ascending() {
        // Arrange
        let publisher = priority(128, 128, GroupOrder::Publisher, 3, 0);
        let ascending = priority(128, 128, GroupOrder::Ascending, 3, 0);
        // Act / Assert
        assert_eq!(publisher, ascending);
    }

    #[test]
    fn group_order_dominates_subgroup_id() {
        // Arrange
        let earlier_group_high_subgroup = priority(128, 128, GroupOrder::Ascending, 0, 7);
        let later_group_first_subgroup = priority(128, 128, GroupOrder::Ascending, 1, 0);
        // Act / Assert
        assert!(earlier_group_high_subgroup > later_group_first_subgroup);
    }

    #[test]
    fn lower_subgroup_id_is_sent_first_within_a_group() {
        // Arrange
        let first = priority(128, 128, GroupOrder::Ascending, 5, 0);
        let second = priority(128, 128, GroupOrder::Ascending, 5, 1);
        // Act / Assert
        assert!(first > second);
    }

    #[test]
    fn subgroup_ids_beyond_the_field_width_tie() {
        // Arrange
        let saturated = priority(128, 128, GroupOrder::Ascending, 0, 7);
        let beyond = priority(128, 128, GroupOrder::Ascending, 0, 1_000);
        // Act / Assert
        assert_eq!(saturated, beyond);
    }

    #[test]
    fn group_rank_wraps_every_4096_groups() {
        // Arrange
        let first = priority(128, 128, GroupOrder::Ascending, 0, 0);
        let wrapped = priority(128, 128, GroupOrder::Ascending, 4096, 0);
        // Act / Assert
        assert_eq!(first, wrapped);
    }

    #[test]
    fn every_data_stream_stays_below_the_control_stream_default() {
        // Arrange
        let highest = priority(0, 0, GroupOrder::Ascending, 0, 0);
        let lowest = priority(255, 255, GroupOrder::Ascending, 4095, u64::MAX);
        // Act / Assert
        assert_eq!(highest, -1);
        assert_eq!(lowest, i32::MIN);
    }
}
