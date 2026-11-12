/// Decides which NEW_GROUP_REQUESTs of a track's subscribers go upstream
/// (draft-18 §10.2.13).
#[derive(Default)]
pub(crate) struct NewGroupRequestGate {
    last_sent: Option<SentRequest>,
}

struct SentRequest {
    new_group_request: u64,
    /// The Largest Group the request is outstanding at. Sent in a
    /// REQUEST_UPDATE: the track's Largest Group at that time. Carried by
    /// the upstream SUBSCRIBEs: the largest one their SUBSCRIBE_OKs reported
    /// so far.
    largest_group: Option<u64>,
    /// Whether the upstream SUBSCRIBEs that create the track carried it.
    in_subscribe: bool,
}

impl NewGroupRequestGate {
    /// Whether the relay sends `new_group_request` upstream while the
    /// track's Largest Group is `largest_group`; one it sends is recorded.
    /// Not sent: a request for a group that already began, and one no
    /// greater than a request still outstanding, which the last one sent is
    /// until the Largest Group increases.
    pub(crate) fn admit(&mut self, new_group_request: u64, largest_group: Option<u64>) -> bool {
        if new_group_request != 0 && largest_group.is_some_and(|group| new_group_request <= group) {
            return false;
        }
        let outstanding = self.last_sent.as_ref().is_some_and(|sent| {
            sent.largest_group == largest_group && sent.new_group_request >= new_group_request
        });
        if outstanding {
            return false;
        }
        self.last_sent = Some(SentRequest {
            new_group_request,
            largest_group,
            in_subscribe: false,
        });
        true
    }

    /// Records the request the upstream SUBSCRIBEs that create the track
    /// carry, sent without asking `admit` and before the track has a Largest
    /// Group; `subscribe_answered` sets the Largest Group it is outstanding
    /// at.
    pub(crate) fn record_in_subscribe(&mut self, new_group_request: u64) {
        self.last_sent = Some(SentRequest {
            new_group_request,
            largest_group: None,
            in_subscribe: true,
        });
    }

    /// The SUBSCRIBE_OK of an upstream SUBSCRIBE that carried the recorded
    /// request reported `largest_group`: the request is outstanding until
    /// the track gets beyond it, since the publisher had that group when it
    /// was asked. A request sent since is left as it is.
    pub(crate) fn subscribe_answered(&mut self, largest_group: Option<u64>) {
        if let Some(sent) = &mut self.last_sent
            && sent.in_subscribe
        {
            sent.largest_group = sent.largest_group.max(largest_group);
        }
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
        let admitted = gate.admit(7, LARGEST_GROUP);

        // Assert
        assert!(!admitted);
    }

    #[test]
    fn a_request_without_group_information_is_sent_whatever_the_largest_group() {
        // Arrange
        let mut gate = NewGroupRequestGate::default();

        // Act
        let admitted = gate.admit(0, LARGEST_GROUP);

        // Assert
        assert!(admitted);
    }

    #[test]
    fn a_request_is_outstanding_until_the_largest_group_increases() {
        // Arrange: the publisher has not started the group the request asked for
        let mut gate = NewGroupRequestGate::default();
        gate.admit(8, LARGEST_GROUP);

        // Act
        let same_request = gate.admit(8, LARGEST_GROUP);
        let smaller_request = gate.admit(0, LARGEST_GROUP);
        let greater_request = gate.admit(9, LARGEST_GROUP);

        // Assert
        assert!(!same_request);
        assert!(!smaller_request);
        assert!(greater_request);
    }

    #[test]
    fn a_request_is_sent_as_soon_as_the_group_the_last_one_asked_for_began() {
        // Arrange: the earlier request started group 8
        let mut gate = NewGroupRequestGate::default();
        gate.admit(8, LARGEST_GROUP);

        // Act
        let for_the_group_after = gate.admit(9, Some(8));
        let without_group_information = gate.admit(0, Some(8));

        // Assert
        assert!(for_the_group_after);
        assert!(!without_group_information);
    }

    #[test]
    fn the_request_of_the_upstream_subscribe_is_outstanding_until_the_track_gets_beyond_its_subscribe_ok()
     {
        // Arrange: the publisher was in group 7 when the SUBSCRIBE asked it
        let mut gate = NewGroupRequestGate::default();
        gate.record_in_subscribe(0);
        gate.subscribe_answered(LARGEST_GROUP);

        // Act
        let in_the_same_group = gate.admit(0, LARGEST_GROUP);
        let in_the_next_group = gate.admit(0, Some(8));

        // Assert
        assert!(!in_the_same_group);
        assert!(in_the_next_group);
    }

    #[test]
    fn the_request_of_an_upstream_subscribe_to_a_track_without_content_is_answered_by_the_first_group()
     {
        // Arrange
        let mut gate = NewGroupRequestGate::default();
        gate.record_in_subscribe(0);
        gate.subscribe_answered(None);

        // Act
        let before_any_group = gate.admit(0, None);
        let in_the_first_group = gate.admit(0, LARGEST_GROUP);

        // Assert
        assert!(!before_any_group);
        assert!(in_the_first_group);
    }

    #[test]
    fn the_request_of_upstream_subscribes_is_outstanding_at_the_largest_group_any_of_them_told() {
        // Arrange: two publishers answer, in groups 9 and 7
        let mut gate = NewGroupRequestGate::default();
        gate.record_in_subscribe(0);
        gate.subscribe_answered(Some(9));
        gate.subscribe_answered(LARGEST_GROUP);

        // Act
        let admitted = gate.admit(0, Some(9));

        // Assert
        assert!(!admitted);
    }

    #[test]
    fn a_late_subscribe_ok_leaves_a_request_sent_since_as_it_is() {
        // Arrange: the track got to group 8, where another request was sent
        let mut gate = NewGroupRequestGate::default();
        gate.record_in_subscribe(0);
        gate.subscribe_answered(LARGEST_GROUP);
        gate.admit(0, Some(8));

        // Act: a second publisher answers the SUBSCRIBE in group 9
        gate.subscribe_answered(Some(9));
        let admitted = gate.admit(0, Some(9));

        // Assert: the request sent in group 8 is no longer outstanding
        assert!(admitted);
    }
}
