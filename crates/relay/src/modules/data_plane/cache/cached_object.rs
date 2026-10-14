use std::time::Duration;

use bytes::Bytes;
use moqt::object::{ExtensionHeaders, ForwardingPreference, Object, ObjectStatus, SubgroupStart};
use tokio::time::Instant;

use crate::modules::data_plane::cache::subgroup_key::SubgroupKey;

/// draft-14 §10.2.1 canonical object as stored by the relay cache.
#[derive(Clone, Debug)]
pub(crate) struct CachedObject {
    pub(crate) object: Object,
    pub(crate) received_at: Instant,
    /// draft-14 §9.2.1.3: from this instant on, the object must not start to
    /// be forwarded. Set when the subscription or fetch it arrived on carried
    /// MAX_CACHE_DURATION.
    pub(crate) expires_at: Option<Instant>,
}

impl CachedObject {
    pub(crate) fn from_object(object: Object) -> Self {
        Self {
            object,
            received_at: Instant::now(),
            expires_at: None,
        }
    }

    pub(crate) fn expiring_after(mut self, max_cache_duration: Option<Duration>) -> Self {
        self.expires_at = max_cache_duration.map(|duration| self.received_at + duration);
        self
    }

    /// The object to send downstream.
    pub(crate) async fn load(&self) -> anyhow::Result<Object> {
        Ok(self.object.clone())
    }

    pub(crate) fn payload_len(&self) -> usize {
        self.object.payload.len()
    }

    pub(crate) fn is_expired(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|expires_at| now >= expires_at)
    }

    /// The End of Group status object a FIN implies for `start`'s subgroup.
    pub(crate) fn end_of_group(start: &SubgroupStart, object_id: u64) -> Self {
        Self::from_object(Object {
            location: moqt::object::Location {
                group_id: start.group_id,
                object_id,
            },
            forwarding: ForwardingPreference::Subgroup {
                subgroup_id: start.subgroup_id,
            },
            publisher_priority: start.publisher_priority,
            status: ObjectStatus::EndOfGroup,
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::new(),
            ends_group: false,
        })
    }

    pub(crate) fn subgroup_key(&self) -> SubgroupKey {
        match self.object.forwarding {
            ForwardingPreference::Subgroup { subgroup_id } => SubgroupKey::Stream {
                group_id: self.object.location.group_id,
                subgroup_id,
            },
            ForwardingPreference::Datagram => SubgroupKey::Datagram {
                group_id: self.object.location.group_id,
            },
        }
    }

    /// draft-14 §8.1: a duplicate whose Forwarding Preference, Subgroup ID,
    /// Priority or Payload differ, or whose Status moves between Normal,
    /// End of Group and End of Track, makes the track Malformed. Extension
    /// header changes and transitions involving Does Not Exist are allowed.
    /// A datagram's `ends_group` is not a §10.2.1 property and FETCH cannot
    /// carry it, so it is never compared.
    pub(crate) fn conflicts_with(&self, other: &Self) -> bool {
        if self.forwarding_conflicts_with(other)
            || self.object.publisher_priority != other.object.publisher_priority
        {
            return true;
        }
        if self.object.status == ObjectStatus::DoesNotExist
            || other.object.status == ObjectStatus::DoesNotExist
        {
            return false;
        }
        self.object.status != other.object.status || self.object.payload != other.object.payload
    }

    /// FETCH carries no forwarding preference; a fetched copy of a datagram
    /// object arrives as a subgroup object whose Subgroup ID is its Object ID
    /// (draft-14 §10.4.4), so that pairing is the same object, not a conflict.
    fn forwarding_conflicts_with(&self, other: &Self) -> bool {
        match (self.object.forwarding, other.object.forwarding) {
            (
                ForwardingPreference::Subgroup { subgroup_id: a },
                ForwardingPreference::Subgroup { subgroup_id: b },
            ) => a != b,
            (ForwardingPreference::Datagram, ForwardingPreference::Datagram) => false,
            (ForwardingPreference::Subgroup { subgroup_id }, ForwardingPreference::Datagram)
            | (ForwardingPreference::Datagram, ForwardingPreference::Subgroup { subgroup_id }) => {
                subgroup_id != self.object.location.object_id
            }
        }
    }

    pub(crate) fn is_datagram(&self) -> bool {
        self.object.forwarding == ForwardingPreference::Datagram
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::test_support::relay_harness::fixtures::cached_object::{
        datagram_object, status_object, stream_object, stream_object_in_subgroup,
        stream_object_with_payload,
    };
    use moqt::object::{KeyValuePair, VariantType};

    fn extension_headers() -> ExtensionHeaders {
        ExtensionHeaders::new(vec![KeyValuePair {
            key: 2,
            value: VariantType::Even(7),
        }])
    }

    #[test]
    fn identical_duplicate_is_identical() {
        // Arrange / Act / Assert
        assert!(!stream_object(0, 0).conflicts_with(&stream_object(0, 0)));
    }

    #[test]
    fn differing_payload_is_a_conflict() {
        // Arrange
        let first = stream_object_with_payload(0, 0, Bytes::from_static(b"a"));
        let second = stream_object_with_payload(0, 0, Bytes::from_static(b"b"));
        // Act / Assert
        assert!(first.conflicts_with(&second));
    }

    #[test]
    fn differing_priority_is_a_conflict() {
        // Arrange
        let first = stream_object(0, 0);
        let mut second = stream_object(0, 0);
        second.object.publisher_priority = first.object.publisher_priority + 1;
        // Act / Assert
        assert!(first.conflicts_with(&second));
    }

    #[test]
    fn differing_subgroup_or_forwarding_preference_is_a_conflict() {
        // Arrange / Act / Assert
        assert!(stream_object(0, 0).conflicts_with(&stream_object_in_subgroup(0, 1, 0)));
        assert!(stream_object(0, 3).conflicts_with(&datagram_object(0, 3)));
    }

    #[test]
    fn fetched_copy_of_a_datagram_object_is_not_a_conflict() {
        // Arrange: §10.4.4 — the fetched copy carries subgroup id = object id
        let fetched = stream_object_in_subgroup(0, 5, 5);
        let live = datagram_object(0, 5);
        // Act / Assert: both orders
        assert!(!fetched.conflicts_with(&live));
        assert!(!live.conflicts_with(&fetched));
    }

    #[test]
    fn subgroup_object_with_another_subgroup_id_conflicts_with_a_datagram() {
        // Arrange
        let subgroup = stream_object_in_subgroup(0, 1, 5);
        let live = datagram_object(0, 5);
        // Act / Assert
        assert!(subgroup.conflicts_with(&live));
    }

    #[test]
    fn differing_extension_headers_are_not_a_conflict() {
        // Arrange: §8.1 allows extension headers to be added, removed or updated
        let first = stream_object(0, 0);
        let mut second = stream_object(0, 0);
        second.object.extension_headers = extension_headers();
        // Act / Assert
        assert!(!first.conflicts_with(&second));
    }

    #[test]
    fn transition_to_or_from_does_not_exist_is_not_a_conflict() {
        // Arrange
        let normal = stream_object(0, 0);
        let missing = status_object(0, 0, ObjectStatus::DoesNotExist);
        // Act / Assert: either arrival order is a tolerated duplicate
        assert!(!normal.conflicts_with(&missing));
        assert!(!missing.conflicts_with(&normal));
    }

    #[test]
    fn transition_between_normal_and_end_of_group_is_a_conflict() {
        // Arrange
        let normal = stream_object_with_payload(0, 0, Bytes::new());
        let end_of_group = status_object(0, 0, ObjectStatus::EndOfGroup);
        // Act / Assert
        assert!(normal.conflicts_with(&end_of_group));
    }
}
