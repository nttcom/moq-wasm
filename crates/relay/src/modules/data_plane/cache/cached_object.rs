use bytes::Bytes;
use moqt::{
    fetch::{FetchObject, FetchObjectField},
    object::{ExtensionHeaders, Object, ObjectStatus, SubgroupStart},
};
use tokio::time::Instant;

use crate::modules::data_plane::cache::subgroup_key::SubgroupKey;

pub(crate) use moqt::object::ForwardingPreference;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SubgroupHeaderFields {
    pub(crate) group_id: u64,
    pub(crate) subgroup_id: u64,
    pub(crate) publisher_priority: u8,
}

impl From<SubgroupStart> for SubgroupHeaderFields {
    fn from(start: SubgroupStart) -> Self {
        Self {
            group_id: start.group_id,
            subgroup_id: start.subgroup_id,
            publisher_priority: start.publisher_priority,
        }
    }
}

impl SubgroupHeaderFields {
    pub(crate) fn key(&self) -> SubgroupKey {
        SubgroupKey::Stream {
            group_id: self.group_id,
            subgroup_id: self.subgroup_id,
        }
    }
}

/// draft-14 §10.2.1 canonical object as stored by the relay cache.
#[derive(Clone, Debug)]
pub(crate) struct CachedObject {
    pub(crate) location: moqt::object::Location,
    pub(crate) forwarding: ForwardingPreference,
    pub(crate) publisher_priority: u8,
    pub(crate) status: ObjectStatus,
    pub(crate) extension_headers: ExtensionHeaders,
    pub(crate) payload: Bytes,
    pub(crate) received_at: Instant,
}

impl CachedObject {
    pub(crate) fn from_object(object: Object) -> Self {
        Self {
            location: object.location,
            forwarding: object.forwarding,
            publisher_priority: object.publisher_priority,
            status: object.status,
            extension_headers: object.extension_headers,
            payload: object.payload,
            received_at: Instant::now(),
        }
    }

    pub(crate) fn to_object(&self) -> Object {
        Object {
            location: self.location,
            forwarding: self.forwarding,
            publisher_priority: self.publisher_priority,
            status: self.status,
            extension_headers: self.extension_headers.clone(),
            payload: self.payload.clone(),
        }
    }

    pub(crate) fn end_of_group(header: &SubgroupHeaderFields, object_id: u64) -> Self {
        Self {
            status: ObjectStatus::EndOfGroup,
            ..Self::empty_at(header, object_id)
        }
    }

    fn empty_at(header: &SubgroupHeaderFields, object_id: u64) -> Self {
        Self {
            location: moqt::object::Location {
                group_id: header.group_id,
                object_id,
            },
            forwarding: ForwardingPreference::Subgroup {
                subgroup_id: header.subgroup_id,
            },
            publisher_priority: header.publisher_priority,
            status: ObjectStatus::Normal,
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::new(),
            received_at: Instant::now(),
        }
    }

    pub(crate) fn from_fetch_object(field: FetchObjectField) -> Self {
        let (status, payload) = match field.fetch_object {
            FetchObject::Payload(payload) => (ObjectStatus::Normal, payload),
            FetchObject::Status(status) => (status, Bytes::new()),
        };
        Self {
            location: moqt::object::Location {
                group_id: field.group_id,
                object_id: field.object_id,
            },
            forwarding: ForwardingPreference::Subgroup {
                subgroup_id: field.subgroup_id,
            },
            publisher_priority: field.publisher_priority,
            status,
            extension_headers: field.extension_headers,
            payload,
            received_at: Instant::now(),
        }
    }

    pub(crate) fn subgroup_key(&self) -> SubgroupKey {
        match self.forwarding {
            ForwardingPreference::Subgroup { subgroup_id } => SubgroupKey::Stream {
                group_id: self.location.group_id,
                subgroup_id,
            },
            ForwardingPreference::Datagram => SubgroupKey::Datagram {
                group_id: self.location.group_id,
            },
        }
    }

    /// draft-14 §8.1: a duplicate whose Forwarding Preference, Subgroup ID,
    /// Priority or Payload differ, or whose Status moves between Normal,
    /// End of Group and End of Track, makes the track Malformed. Extension
    /// header changes and transitions involving Does Not Exist are allowed.
    pub(crate) fn conflicts_with(&self, other: &Self) -> bool {
        if self.forwarding_conflicts_with(other)
            || self.publisher_priority != other.publisher_priority
        {
            return true;
        }
        if self.status == ObjectStatus::DoesNotExist || other.status == ObjectStatus::DoesNotExist {
            return false;
        }
        self.status != other.status || self.payload != other.payload
    }

    /// FETCH carries no forwarding preference; a fetched copy of a datagram
    /// object arrives as a subgroup object whose Subgroup ID is its Object ID
    /// (draft-14 §10.4.4), so that pairing is the same object, not a conflict.
    fn forwarding_conflicts_with(&self, other: &Self) -> bool {
        match (self.forwarding, other.forwarding) {
            (
                ForwardingPreference::Subgroup { subgroup_id: a },
                ForwardingPreference::Subgroup { subgroup_id: b },
            ) => a != b,
            (ForwardingPreference::Datagram, ForwardingPreference::Datagram) => false,
            (ForwardingPreference::Subgroup { subgroup_id }, ForwardingPreference::Datagram)
            | (ForwardingPreference::Datagram, ForwardingPreference::Subgroup { subgroup_id }) => {
                subgroup_id != self.location.object_id
            }
        }
    }

    pub(crate) fn is_datagram(&self) -> bool {
        self.forwarding == ForwardingPreference::Datagram
    }

    pub(crate) fn to_fetch_object_field(&self) -> FetchObjectField {
        let subgroup_id = match self.forwarding {
            ForwardingPreference::Subgroup { subgroup_id } => subgroup_id,
            ForwardingPreference::Datagram => self.location.object_id,
        };
        let fetch_object = if self.payload.is_empty() {
            FetchObject::Status(self.status)
        } else {
            FetchObject::Payload(self.payload.clone())
        };
        FetchObjectField::new(
            self.location.group_id,
            subgroup_id,
            self.location.object_id,
            self.publisher_priority,
            self.extension_headers.clone(),
            fetch_object,
        )
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
        let second = CachedObject {
            publisher_priority: first.publisher_priority + 1,
            ..stream_object(0, 0)
        };
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
        let second = CachedObject {
            extension_headers: extension_headers(),
            ..stream_object(0, 0)
        };
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

    #[test]
    fn fetch_field_of_datagram_object_uses_object_id_as_subgroup_id() {
        // Arrange / Act
        let field = datagram_object(1, 7).to_fetch_object_field();
        // Assert: §10.4.4
        assert_eq!(field.subgroup_id, 7);
        assert_eq!(field.object_id, 7);
    }

    #[test]
    fn empty_payload_is_encoded_as_explicit_status() {
        // Arrange: a zero-length Normal object must encode Object Status 0x0
        let cached = stream_object_with_payload(0, 0, Bytes::new());
        // Act
        let fetch_field = cached.to_fetch_object_field();
        // Assert
        assert_eq!(
            fetch_field.fetch_object,
            FetchObject::Status(ObjectStatus::Normal)
        );
    }

    #[test]
    fn fetch_object_converts_one_to_one() {
        // Arrange
        let field = FetchObjectField::new(
            3,
            1,
            4,
            200,
            extension_headers(),
            FetchObject::Payload(Bytes::from_static(b"p")),
        );
        // Act
        let cached = CachedObject::from_fetch_object(field.clone());
        // Assert
        assert_eq!(
            cached.location,
            moqt::object::Location {
                group_id: 3,
                object_id: 4
            }
        );
        assert_eq!(
            cached.forwarding,
            ForwardingPreference::Subgroup { subgroup_id: 1 }
        );
        assert_eq!(cached.publisher_priority, 200);
        assert_eq!(cached.to_fetch_object_field(), field);
    }
}
