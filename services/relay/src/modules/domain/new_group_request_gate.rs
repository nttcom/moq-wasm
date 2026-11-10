use std::time::Duration;

use tokio::time::Instant;

/// The least time between two NEW_GROUP_REQUESTs the relay sends upstream for
/// one track. A subscriber whose request falls within it joins by FETCH: the
/// group the earlier request started is at most this old.
pub(crate) const NEW_GROUP_REQUEST_INTERVAL: Duration = Duration::from_secs(2);

/// Decides which NEW_GROUP_REQUESTs of a track's subscribers go upstream
/// (draft-18 §10.2.13).
#[derive(Default)]
pub(crate) struct NewGroupRequestGate {
    last_sent: Option<SentRequest>,
}

struct SentRequest {
    at: Instant,
    new_group_request: u64,
    /// The track's Largest Group when the request was sent.
    largest_group: Option<u64>,
}

impl NewGroupRequestGate {
    /// Whether the relay sends `new_group_request` upstream at `now`, while
    /// the track's Largest Group is `largest_group`; one it sends is
    /// recorded. Not sent: a request for a group that already began, one
    /// that follows the last one sent within `NEW_GROUP_REQUEST_INTERVAL`,
    /// and one no greater than a request still outstanding, which the last
    /// one sent is until the Largest Group increases.
    pub(crate) fn admit(
        &mut self,
        new_group_request: u64,
        largest_group: Option<u64>,
        now: Instant,
    ) -> bool {
        if new_group_request != 0 && largest_group.is_some_and(|group| new_group_request <= group) {
            return false;
        }
        if let Some(sent) = &self.last_sent {
            let outstanding =
                sent.largest_group == largest_group && sent.new_group_request >= new_group_request;
            if outstanding || now.duration_since(sent.at) < NEW_GROUP_REQUEST_INTERVAL {
                return false;
            }
        }
        self.record(new_group_request, largest_group, now);
        true
    }

    /// Records a request sent without asking `admit`: the one the upstream
    /// SUBSCRIBE that creates the track carries.
    pub(crate) fn record(
        &mut self,
        new_group_request: u64,
        largest_group: Option<u64>,
        now: Instant,
    ) {
        self.last_sent = Some(SentRequest {
            at: now,
            new_group_request,
            largest_group,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LARGEST_GROUP: Option<u64> = Some(7);

    #[test]
    fn a_request_for_a_group_that_began_is_not_sent() {
        // Arrange
        let mut gate = NewGroupRequestGate::default();

        // Act
        let admitted = gate.admit(7, LARGEST_GROUP, Instant::now());

        // Assert
        assert!(!admitted);
    }

    #[test]
    fn a_request_without_group_information_is_sent_whatever_the_largest_group() {
        // Arrange
        let mut gate = NewGroupRequestGate::default();

        // Act
        let admitted = gate.admit(0, LARGEST_GROUP, Instant::now());

        // Assert
        assert!(admitted);
    }

    #[test]
    fn a_request_within_the_interval_of_the_last_one_sent_is_not_sent() {
        // Arrange: the earlier request started group 8
        let mut gate = NewGroupRequestGate::default();
        let sent_at = Instant::now();
        gate.admit(8, LARGEST_GROUP, sent_at);
        let just_before = sent_at + NEW_GROUP_REQUEST_INTERVAL - Duration::from_millis(1);

        // Act
        let within_interval = gate.admit(9, Some(8), just_before);
        let after_interval = gate.admit(9, Some(8), sent_at + NEW_GROUP_REQUEST_INTERVAL);

        // Assert
        assert!(!within_interval);
        assert!(after_interval);
    }

    #[test]
    fn a_request_is_outstanding_until_the_largest_group_increases() {
        // Arrange: the publisher has not started the group the request asked for
        let mut gate = NewGroupRequestGate::default();
        let sent_at = Instant::now();
        gate.admit(8, LARGEST_GROUP, sent_at);
        let later = sent_at + NEW_GROUP_REQUEST_INTERVAL;

        // Act
        let same_request = gate.admit(8, LARGEST_GROUP, later);
        let greater_request = gate.admit(9, LARGEST_GROUP, later);

        // Assert
        assert!(!same_request);
        assert!(greater_request);
    }

    #[test]
    fn the_request_of_the_upstream_subscribe_holds_back_the_ones_that_follow() {
        // Arrange
        let mut gate = NewGroupRequestGate::default();
        let sent_at = Instant::now();
        gate.record(0, None, sent_at);

        // Act
        let admitted = gate.admit(0, LARGEST_GROUP, sent_at + Duration::from_secs(1));

        // Assert
        assert!(!admitted);
    }
}
