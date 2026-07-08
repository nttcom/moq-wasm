/// Derives the transport-level stream priority from MoQT priorities.
///
/// MoQT priority numbers are 0-255 where a lower number means higher
/// precedence, and the subscriber priority is considered before the
/// publisher priority (draft-ietf-moq-transport-14 §7.2). QUIC send
/// stream priorities are the opposite: a higher `i32` is sent first.
///
/// The result is always negative so that the control stream, which
/// keeps the transport default priority of 0, outranks every data
/// stream (§7.2 recommends prioritizing the control stream highest).
pub(crate) fn resolve_transport_priority(subscriber_priority: u8, publisher_priority: u8) -> i32 {
    -1 - (((subscriber_priority as i32) << 8) | publisher_priority as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_subscriber_priority_is_sent_first() {
        // Arrange: two streams differing only in subscriber priority
        let high = resolve_transport_priority(0, 128);
        let low = resolve_transport_priority(255, 128);
        // Assert: the lower MoQT number maps to the higher transport priority
        assert!(high > low);
    }

    #[test]
    fn subscriber_priority_dominates_publisher_priority() {
        // Arrange: better subscriber priority paired with the worst publisher
        // priority, versus the reverse
        let subscriber_wins = resolve_transport_priority(10, 255);
        let publisher_only = resolve_transport_priority(11, 0);
        // Assert: subscriber priority is compared first
        assert!(subscriber_wins > publisher_only);
    }

    #[test]
    fn publisher_priority_breaks_ties() {
        // Arrange: equal subscriber priorities, different publisher priorities
        let high = resolve_transport_priority(128, 0);
        let low = resolve_transport_priority(128, 255);
        // Assert: the lower publisher number is sent first
        assert!(high > low);
    }

    #[test]
    fn data_streams_stay_below_control_stream_default() {
        // Assert: even the highest-priority data stream stays below the
        // control stream's transport default of 0
        assert!(resolve_transport_priority(0, 0) < 0);
        assert_eq!(resolve_transport_priority(255, 255), -65536);
    }
}
