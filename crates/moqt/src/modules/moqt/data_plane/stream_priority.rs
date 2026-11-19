const GROUP_POSITION_BITS: u32 = 12;
const GROUP_POSITION_MAX: u64 = (1 << GROUP_POSITION_BITS) - 1;
const SUBGROUP_BITS: u32 = 3;
const SUBGROUP_MAX: u64 = (1 << SUBGROUP_BITS) - 1;
const GROUP_POSITION_SHIFT: u32 = SUBGROUP_BITS;
const PUBLISHER_PRIORITY_SHIFT: u32 = GROUP_POSITION_SHIFT + GROUP_POSITION_BITS;
const SUBSCRIBER_PRIORITY_SHIFT: u32 = PUBLISHER_PRIORITY_SHIFT + 8;

/// draft-14 §7.2 scheduling inputs of one subgroup stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamPriority {
    pub subscriber_priority: u8,
    pub publisher_priority: u8,
    /// How many groups of the subscription its Group Order sends before this
    /// one; 0 for the next to send. Equal positions of subscriptions with
    /// equal priorities take turns. Positions past 4095 tie.
    pub group_position: u64,
    pub subgroup_id: u64,
}

impl StreamPriority {
    pub fn transport_priority(&self) -> i32 {
        let key = (u64::from(self.subscriber_priority) << SUBSCRIBER_PRIORITY_SHIFT)
            | (u64::from(self.publisher_priority) << PUBLISHER_PRIORITY_SHIFT)
            | (self.group_position.min(GROUP_POSITION_MAX) << GROUP_POSITION_SHIFT)
            | self.subgroup_id.min(SUBGROUP_MAX);
        -1 - key as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn priority(
        subscriber_priority: u8,
        publisher_priority: u8,
        group_position: u64,
        subgroup_id: u64,
    ) -> i32 {
        StreamPriority {
            subscriber_priority,
            publisher_priority,
            group_position,
            subgroup_id,
        }
        .transport_priority()
    }

    #[test]
    fn lower_subscriber_priority_number_is_sent_first() {
        // Arrange
        let high = priority(0, 128, 0, 0);
        let low = priority(255, 128, 0, 0);
        // Act / Assert
        assert!(high > low);
    }

    #[test]
    fn subscriber_priority_dominates_publisher_priority() {
        // Arrange
        let better_subscriber = priority(10, 255, 4095, 7);
        let better_publisher = priority(11, 0, 0, 0);
        // Act / Assert
        assert!(better_subscriber > better_publisher);
    }

    #[test]
    fn publisher_priority_dominates_group_position() {
        // Arrange
        let better_publisher = priority(128, 0, 4095, 7);
        let next_group = priority(128, 1, 0, 0);
        // Act / Assert
        assert!(better_publisher > next_group);
    }

    #[test]
    fn the_next_group_of_a_subscription_is_sent_first() {
        // Arrange
        let next = priority(128, 128, 0, 0);
        let after = priority(128, 128, 1, 0);
        // Act / Assert
        assert!(next > after);
    }

    #[test]
    fn group_position_dominates_subgroup_id() {
        // Arrange
        let next_group_high_subgroup = priority(128, 128, 0, 7);
        let later_group_first_subgroup = priority(128, 128, 1, 0);
        // Act / Assert
        assert!(next_group_high_subgroup > later_group_first_subgroup);
    }

    #[test]
    fn lower_subgroup_id_is_sent_first_within_a_group() {
        // Arrange
        let first = priority(128, 128, 5, 0);
        let second = priority(128, 128, 5, 1);
        // Act / Assert
        assert!(first > second);
    }

    #[test]
    fn subgroup_ids_beyond_the_field_width_tie() {
        // Arrange
        let saturated = priority(128, 128, 0, 7);
        let beyond = priority(128, 128, 0, 1_000);
        // Act / Assert
        assert_eq!(saturated, beyond);
    }

    #[test]
    fn group_positions_beyond_the_field_width_tie() {
        // Arrange
        let saturated = priority(128, 128, 4095, 0);
        let beyond = priority(128, 128, 10_000, 0);
        // Act / Assert
        assert_eq!(saturated, beyond);
    }

    #[test]
    fn every_data_stream_stays_below_the_control_stream_default() {
        // Arrange
        let highest = priority(0, 0, 0, 0);
        let lowest = priority(255, 255, 4095, u64::MAX);
        // Act / Assert
        assert_eq!(highest, -1);
        assert_eq!(lowest, i32::MIN);
    }
}
