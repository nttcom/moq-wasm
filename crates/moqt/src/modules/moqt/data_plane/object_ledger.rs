use std::{
    collections::{HashMap, hash_map::Entry},
    hash::{BuildHasher, DefaultHasher, Hasher, RandomState},
};

use crate::modules::moqt::{
    control_plane::control_messages::messages::parameters::location::Location,
    data_plane::{
        malformed_track_error::MalformedTrackError,
        object::{
            canonical_object::{ForwardingPreference, Object},
            object_status::ObjectStatus,
        },
    },
    version::Version,
};

/// The objects of one track a session has received, kept while a
/// subscription of the track is read or a FETCH response the application
/// took has not ended. No entry is ever removed, so
/// the ledger grows by one entry per object for that whole time: 32 bytes
/// with its key, about 38 with the map's control byte at its 7/8 load, so
/// about 260 MB a day for a track of 80 objects/s. An object
/// that arrives again with another forwarding preference, subgroup, priority
/// or payload makes the track malformed (draft-14 §8.1, draft-18 §9.1), and
/// so does, on draft-14 only, a status that moves between Normal, End of
/// Group and End of Track. Its extension headers may change, and so may its
/// status to or from Does Not Exist.
pub(crate) struct ObjectLedger {
    hasher: RandomState,
    objects: std::sync::Mutex<HashMap<Location, Recorded>>,
}

/// What a duplicate is compared on, in 16 bytes, as the ledger keeps one
/// per object: the subgroup id exactly, and the priority, status and a
/// 52-bit hash of the payload packed into one word. A different payload
/// goes unnoticed only when its hash collides, at odds of 2^-52.
#[derive(Clone, Copy)]
struct Recorded {
    /// The Subgroup ID, or `DATAGRAM`; a Subgroup ID is a varint, below 2^62.
    subgroup: u64,
    /// `PAYLOAD_HASH` | priority << `PRIORITY_SHIFT` | status <<
    /// `STATUS_SHIFT` | `FORWARDING_INFERRED`.
    packed: u64,
}

const DATAGRAM: u64 = u64::MAX;
const PAYLOAD_HASH: u64 = (1 << 52) - 1;
const PRIORITY_SHIFT: u32 = 52;
const STATUS_SHIFT: u32 = 60;
/// From a draft-14 FETCH, which carries no forwarding preference.
const FORWARDING_INFERRED: u64 = 1 << 63;

impl std::fmt::Debug for ObjectLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectLedger").finish_non_exhaustive()
    }
}

impl ObjectLedger {
    pub(crate) fn new() -> Self {
        Self {
            hasher: RandomState::new(),
            objects: std::sync::Mutex::default(),
        }
    }

    /// Hashes a payload that arrives in pieces, for `check_hashed`.
    pub(crate) fn payload_hasher(&self) -> PayloadHasher {
        PayloadHasher(self.hasher.build_hasher())
    }

    /// Records `object`, a copy from `source`; fails when an earlier copy
    /// of it differs.
    pub(crate) fn check(
        &self,
        object: &Object,
        source: CopySource,
    ) -> Result<(), MalformedTrackError> {
        let mut payload = self.payload_hasher();
        payload.update(&object.payload);
        self.check_hashed(object, payload, source)
    }

    /// `check` for an object whose payload `payload` has hashed; the
    /// object's own payload is ignored.
    pub(crate) fn check_hashed(
        &self,
        object: &Object,
        payload: PayloadHasher,
        source: CopySource,
    ) -> Result<(), MalformedTrackError> {
        let received = Recorded::new(object, payload.0.finish(), source.forwarding_inferred());
        let mut objects = self
            .objects
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match objects.entry(object.location) {
            Entry::Vacant(slot) => {
                slot.insert(received);
            }
            Entry::Occupied(mut recorded) => {
                let recorded = recorded.get_mut();
                if recorded.conflicts_with(received, object.location.object_id, source) {
                    return Err(MalformedTrackError::ConflictingDuplicate {
                        location: object.location,
                    });
                }
                if recorded.status() == ObjectStatus::DoesNotExist {
                    *recorded = received;
                } else if recorded.forwarding_inferred() && !received.forwarding_inferred() {
                    recorded.subgroup = received.subgroup;
                    recorded.packed &= !FORWARDING_INFERRED;
                }
            }
        }
        Ok(())
    }
}

/// Where a copy of an object came from: its draft decides how the copy is
/// compared with the earlier ones.
#[derive(Clone, Copy, Debug)]
pub(crate) enum CopySource {
    /// A subgroup stream or datagram of a subscription.
    Subscription(Version),
    Fetch(Version),
}

impl CopySource {
    fn version(self) -> Version {
        match self {
            Self::Subscription(version) | Self::Fetch(version) => version,
        }
    }

    /// A draft-14 FETCH carries no forwarding preference, so a datagram
    /// object arrives as a subgroup object whose Subgroup ID is its Object
    /// ID (§10.4.4).
    fn forwarding_inferred(self) -> bool {
        matches!(self, Self::Fetch(version) if !version.fetch_carries_forwarding_preference())
    }
}

/// The hash of a payload fed in pieces; SipHash, behind `DefaultHasher`,
/// hashes a byte stream the same however it is split.
#[derive(Debug)]
pub(crate) struct PayloadHasher(DefaultHasher);

impl PayloadHasher {
    pub(crate) fn update(&mut self, piece: &[u8]) {
        self.0.write(piece);
    }
}

impl Recorded {
    fn new(object: &Object, payload_hash: u64, forwarding_inferred: bool) -> Self {
        let subgroup = match object.forwarding {
            ForwardingPreference::Subgroup { subgroup_id } => subgroup_id,
            ForwardingPreference::Datagram => DATAGRAM,
        };
        let mut packed = (payload_hash & PAYLOAD_HASH)
            | (u64::from(object.publisher_priority) << PRIORITY_SHIFT)
            | (u64::from(u8::from(object.status)) << STATUS_SHIFT);
        if forwarding_inferred {
            packed |= FORWARDING_INFERRED;
        }
        Self { subgroup, packed }
    }

    fn priority(self) -> u8 {
        (self.packed >> PRIORITY_SHIFT) as u8
    }

    fn status(self) -> ObjectStatus {
        let code = (self.packed >> STATUS_SHIFT) as u8 & 0x7;
        ObjectStatus::try_from(code).expect("`new` packs a valid status")
    }

    fn payload_hash(self) -> u64 {
        self.packed & PAYLOAD_HASH
    }

    fn forwarding_inferred(self) -> bool {
        self.packed & FORWARDING_INFERRED != 0
    }

    /// `other` is the copy from `source`, whose draft decides whether the
    /// status is compared.
    fn conflicts_with(self, other: Self, object_id: u64, source: CopySource) -> bool {
        if self.forwarding_conflicts_with(other, object_id) || self.priority() != other.priority() {
            return true;
        }
        let (status, other_status) = (self.status(), other.status());
        if status == ObjectStatus::DoesNotExist || other_status == ObjectStatus::DoesNotExist {
            return false;
        }
        (source.version().compares_duplicate_status() && status != other_status)
            || self.payload_hash() != other.payload_hash()
    }

    /// A draft-14 FETCH copy in the subgroup of its Object ID is the same
    /// object as a datagram.
    fn forwarding_conflicts_with(self, other: Self, object_id: u64) -> bool {
        match (self.subgroup, other.subgroup) {
            (subgroup_id, DATAGRAM) if self.forwarding_inferred() => subgroup_id != object_id,
            (DATAGRAM, subgroup_id) if other.forwarding_inferred() => subgroup_id != object_id,
            (subgroup, other_subgroup) => subgroup != other_subgroup,
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::{CopySource, ObjectLedger, Recorded};
    use crate::{
        fetch::Fetch,
        modules::moqt::version::Version,
        modules::test_support::{
            PublishedTrack, data_receiver, fetch_answered_with, location, next_subgroup,
            object_with_extensions, published_draft18_track, published_track,
            read_subgroup_objects, send_subgroup_object, within,
        },
        object::{
            DataReceiver, ExtensionHeaders, ForwardingPreference, KeyValuePair, Location,
            MalformedTrackError, Object, ObjectStatus, StreamReceiveError, Subgroup, SubgroupPart,
            SubgroupStart, TrackData, VariantType,
        },
    };

    fn conflict(object: &Object) -> Result<(), MalformedTrackError> {
        Err(MalformedTrackError::ConflictingDuplicate {
            location: object.location,
        })
    }

    fn status_object(status: ObjectStatus) -> Object {
        Object {
            status,
            payload: Bytes::new(),
            ..object_with_extensions()
        }
    }

    /// A subgroup or datagram copy on draft-14.
    const LIVE: CopySource = CopySource::Subscription(Version::Draft14);
    const DRAFT18: CopySource = CopySource::Subscription(Version::Draft18);
    const DRAFT14_FETCH: CopySource = CopySource::Fetch(Version::Draft14);
    const DRAFT18_FETCH: CopySource = CopySource::Fetch(Version::Draft18);

    fn check_both(first: &Object, second: &Object) -> Result<(), MalformedTrackError> {
        check_both_from(first, second, LIVE)
    }

    fn check_both_from(
        first: &Object,
        second: &Object,
        source: CopySource,
    ) -> Result<(), MalformedTrackError> {
        let ledger = ObjectLedger::new();
        ledger.check(first, source).unwrap();
        ledger.check(second, source)
    }

    #[test]
    fn a_payload_hashed_in_pieces_matches_the_whole_payload() {
        // Arrange
        let ledger = ObjectLedger::new();
        let object = object_with_extensions();
        ledger.check(&object, LIVE).unwrap();
        let mut pieces = ledger.payload_hasher();
        let (first, rest) = object.payload.split_at(1);
        pieces.update(first);
        pieces.update(rest);

        // Act
        let result = ledger.check_hashed(&object, pieces, LIVE);

        // Assert: the same object, however its payload arrived
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn a_ledger_entry_takes_32_bytes() {
        // Act
        let entry = std::mem::size_of::<(Location, Recorded)>();
        // Assert: with hashbrown's control byte and 7/8 load, about 38 B per object
        assert_eq!(entry, 32);
    }

    #[test]
    fn an_identical_duplicate_passes() {
        // Arrange
        let object = object_with_extensions();
        // Act / Assert
        assert_eq!(check_both(&object, &object), Ok(()));
    }

    #[test]
    fn a_duplicate_with_other_extension_headers_passes() {
        // Arrange
        let first = object_with_extensions();
        let second = Object {
            extension_headers: ExtensionHeaders::new(vec![KeyValuePair {
                key: 0x02,
                value: VariantType::Even(1),
            }]),
            ..first.clone()
        };
        // Act / Assert
        assert_eq!(check_both(&first, &second), Ok(()));
    }

    #[test]
    fn a_duplicate_with_another_payload_is_a_conflict() {
        // Arrange
        let first = object_with_extensions();
        let second = Object {
            payload: Bytes::from_static(b"other"),
            ..first.clone()
        };
        // Act / Assert
        assert_eq!(check_both(&first, &second), conflict(&first));
    }

    #[test]
    fn a_duplicate_with_a_payload_of_the_same_length_is_a_conflict() {
        // Arrange
        let first = Object {
            payload: Bytes::from_static(b"aa"),
            ..object_with_extensions()
        };
        let second = Object {
            payload: Bytes::from_static(b"ab"),
            ..first.clone()
        };
        // Act / Assert
        assert_eq!(check_both(&first, &second), conflict(&first));
    }

    #[test]
    fn a_duplicate_with_another_priority_is_a_conflict() {
        // Arrange
        let first = object_with_extensions();
        let second = Object {
            publisher_priority: first.publisher_priority + 1,
            ..first.clone()
        };
        // Act / Assert
        assert_eq!(check_both(&first, &second), conflict(&first));
    }

    #[test]
    fn a_duplicate_in_another_subgroup_is_a_conflict() {
        // Arrange
        let first = object_with_extensions();
        let second = Object {
            forwarding: ForwardingPreference::Subgroup { subgroup_id: 9 },
            ..first.clone()
        };
        // Act / Assert
        assert_eq!(check_both(&first, &second), conflict(&first));
    }

    #[test]
    fn a_duplicate_with_another_forwarding_preference_is_a_conflict() {
        // Arrange
        let first = object_with_extensions();
        let second = Object {
            forwarding: ForwardingPreference::Datagram,
            ..first.clone()
        };
        // Act / Assert: both orders
        assert_eq!(check_both(&first, &second), conflict(&first));
        assert_eq!(check_both(&second, &first), conflict(&first));
    }

    #[test]
    fn a_draft14_status_moving_between_normal_end_of_group_and_end_of_track_is_a_conflict() {
        let statuses = [
            ObjectStatus::Normal,
            ObjectStatus::EndOfGroup,
            ObjectStatus::EndOfTrack,
        ];
        for first in statuses {
            for second in statuses.into_iter().filter(|second| *second != first) {
                // Arrange
                let (first, second) = (status_object(first), status_object(second));
                // Act / Assert: draft-14 §8.1 item 1
                assert_eq!(
                    check_both(&first, &second),
                    conflict(&first),
                    "{:?} then {:?}",
                    first.status,
                    second.status
                );
            }
        }
    }

    #[test]
    fn a_draft14_status_moving_to_or_from_does_not_exist_passes() {
        // Arrange
        let normal = object_with_extensions();
        let missing = status_object(ObjectStatus::DoesNotExist);
        // Act / Assert: draft-14 §8.1 item 1, both orders
        assert_eq!(check_both(&normal, &missing), Ok(()));
        assert_eq!(check_both(&missing, &normal), Ok(()));
    }

    #[test]
    fn a_draft18_duplicate_with_another_status_passes() {
        let statuses = [
            ObjectStatus::Normal,
            ObjectStatus::EndOfGroup,
            ObjectStatus::EndOfTrack,
        ];
        for first in statuses {
            for second in statuses.into_iter().filter(|second| *second != first) {
                // Arrange
                let (first, second) = (status_object(first), status_object(second));
                // Act / Assert: draft-18 §9.1 compares no status
                assert_eq!(
                    check_both_from(&first, &second, DRAFT18),
                    Ok(()),
                    "{:?} then {:?}",
                    first.status,
                    second.status
                );
            }
        }
    }

    #[test]
    fn a_draft18_duplicate_with_another_preference_subgroup_priority_or_payload_is_a_conflict() {
        // Arrange
        let first = object_with_extensions();
        let copies = [
            Object {
                forwarding: ForwardingPreference::Datagram,
                ..first.clone()
            },
            Object {
                forwarding: ForwardingPreference::Subgroup { subgroup_id: 9 },
                ..first.clone()
            },
            Object {
                publisher_priority: first.publisher_priority + 1,
                ..first.clone()
            },
            Object {
                payload: Bytes::from_static(b"other"),
                ..first.clone()
            },
        ];
        for copy in copies {
            // Act / Assert: draft-18 §9.1
            assert_eq!(
                check_both_from(&first, &copy, DRAFT18),
                conflict(&first),
                "{copy:?}"
            );
        }
    }

    #[test]
    fn an_object_found_missing_is_still_compared_with_its_first_copy() {
        // Arrange
        let ledger = ObjectLedger::new();
        let normal = object_with_extensions();
        ledger.check(&normal, LIVE).unwrap();
        ledger
            .check(&status_object(ObjectStatus::DoesNotExist), LIVE)
            .unwrap();
        let other = Object {
            payload: Bytes::from_static(b"other"),
            ..normal.clone()
        };
        // Act / Assert
        assert_eq!(ledger.check(&other, LIVE), conflict(&normal));
    }

    fn datagram() -> Object {
        Object {
            forwarding: ForwardingPreference::Datagram,
            ..object_with_extensions()
        }
    }

    /// A draft-14 FETCH copy of `object` in subgroup `subgroup_id`.
    fn fetched_in(object: &Object, subgroup_id: u64) -> Object {
        Object {
            forwarding: ForwardingPreference::Subgroup { subgroup_id },
            ..object.clone()
        }
    }

    #[test]
    fn a_draft14_fetched_copy_of_a_datagram_in_the_subgroup_of_its_object_id_passes() {
        // Arrange
        let live = datagram();
        let fetched = fetched_in(&live, live.location.object_id);
        let (live_first, fetched_first) = (ObjectLedger::new(), ObjectLedger::new());
        live_first.check(&live, LIVE).unwrap();
        fetched_first.check(&fetched, DRAFT14_FETCH).unwrap();
        // Act / Assert: both orders
        assert_eq!(live_first.check(&fetched, DRAFT14_FETCH), Ok(()));
        assert_eq!(fetched_first.check(&live, LIVE), Ok(()));
    }

    #[test]
    fn a_draft14_fetched_copy_of_a_datagram_in_another_subgroup_is_a_conflict() {
        // Arrange
        let live = datagram();
        let ledger = ObjectLedger::new();
        ledger.check(&live, LIVE).unwrap();
        // Act / Assert
        assert_eq!(
            ledger.check(
                &fetched_in(&live, live.location.object_id + 1),
                DRAFT14_FETCH
            ),
            conflict(&live)
        );
    }

    #[test]
    fn a_draft14_fetched_copy_is_compared_exactly_once_the_live_object_arrives() {
        // Arrange: the live datagram tells the preference the FETCH copy lacked
        let live = datagram();
        let fetched = fetched_in(&live, live.location.object_id);
        let ledger = ObjectLedger::new();
        ledger.check(&fetched, DRAFT14_FETCH).unwrap();
        ledger.check(&live, LIVE).unwrap();
        // Act / Assert: a subgroup copy with a known preference
        assert_eq!(ledger.check(&fetched, LIVE), conflict(&live));
    }

    #[test]
    fn a_subgroup_copy_of_a_datagram_with_a_known_preference_is_a_conflict() {
        // Arrange: draft-18 FETCH carries the preference
        let live = datagram();
        let ledger = ObjectLedger::new();
        ledger.check(&live, LIVE).unwrap();
        // Act / Assert
        assert_eq!(
            ledger.check(&fetched_in(&live, live.location.object_id), DRAFT18_FETCH),
            conflict(&live)
        );
    }

    #[test]
    fn objects_at_other_locations_are_not_compared() {
        // Arrange
        let first = object_with_extensions();
        let second = Object {
            location: location(first.location.group_id, first.location.object_id + 1),
            payload: Bytes::from_static(b"other"),
            publisher_priority: first.publisher_priority + 1,
            ..first.clone()
        };
        // Act / Assert
        assert_eq!(check_both(&first, &second), Ok(()));
    }

    /// The server side of `track` receives `first`, then `second` on
    /// another subgroup stream; returns how the second stream's object reads.
    async fn receive_second_copy(
        track: &PublishedTrack,
        receiver: &mut DataReceiver,
        first: &Object,
        second: &Object,
    ) -> Result<Option<Subgroup>, StreamReceiveError> {
        within(send_subgroup_object(&track.client, &track.published, first)).await;
        let mut first_stream = within(next_subgroup(receiver)).await;
        let delivered = within(read_subgroup_objects(&mut first_stream)).await;
        assert_eq!(
            delivered
                .iter()
                .map(|object| object.location)
                .collect::<Vec<_>>(),
            [first.location]
        );
        within(send_subgroup_object(
            &track.client,
            &track.published,
            second,
        ))
        .await;
        let mut second_stream = within(next_subgroup(receiver)).await;
        within(second_stream.receive()).await.unwrap();
        within(second_stream.receive()).await
    }

    async fn assert_subgroup_copies(track: PublishedTrack) {
        // Arrange
        let identical = object_with_extensions();
        let first = Object {
            location: location(identical.location.group_id + 1, 0),
            ..identical.clone()
        };
        let other_payload = Object {
            payload: Bytes::from_static(b"other"),
            ..first.clone()
        };
        let mut receiver = data_receiver(&track.server, &track.accepted).await;

        // Act
        let identical_copy =
            receive_second_copy(&track, &mut receiver, &identical, &identical).await;
        let conflicting_copy =
            receive_second_copy(&track, &mut receiver, &first, &other_payload).await;

        // Assert
        assert!(
            matches!(&identical_copy, Ok(Some(Subgroup::Object(object)))
                if object.payload == identical.payload),
            "{identical_copy:?}"
        );
        assert!(
            matches!(
                conflicting_copy,
                Err(StreamReceiveError::MalformedTrack(
                    MalformedTrackError::ConflictingDuplicate { location }
                )) if location == first.location
            ),
            "{conflicting_copy:?}"
        );
    }

    #[tokio::test]
    async fn a_subgroup_object_repeated_with_another_payload_is_a_malformed_track() {
        assert_subgroup_copies(published_track("ledger-subgroup-copies").await).await;
    }

    #[tokio::test]
    async fn a_draft18_subgroup_object_repeated_with_another_payload_is_a_malformed_track() {
        assert_subgroup_copies(published_draft18_track("ledger-subgroup-copies-draft18").await)
            .await;
    }

    /// The second copy of a status object arrives with another status.
    async fn receive_status_change(
        track: PublishedTrack,
    ) -> Result<Option<Subgroup>, StreamReceiveError> {
        let first = status_object(ObjectStatus::EndOfGroup);
        let second = status_object(ObjectStatus::EndOfTrack);
        let mut receiver = data_receiver(&track.server, &track.accepted).await;
        receive_second_copy(&track, &mut receiver, &first, &second).await
    }

    #[tokio::test]
    async fn a_status_change_of_a_received_object_is_a_malformed_track_on_draft14() {
        // Arrange
        let track = published_track("ledger-status-change").await;
        let location = status_object(ObjectStatus::EndOfGroup).location;
        // Act
        let second_copy = receive_status_change(track).await;
        // Assert: draft-14 §8.1 item 1
        assert!(
            matches!(
                second_copy,
                Err(StreamReceiveError::MalformedTrack(
                    MalformedTrackError::ConflictingDuplicate { location: conflicting }
                )) if conflicting == location
            ),
            "{second_copy:?}"
        );
    }

    #[tokio::test]
    async fn a_status_change_of_a_received_object_is_accepted_on_draft18() {
        // Arrange
        let track = published_draft18_track("ledger-status-change-draft18").await;
        // Act
        let second_copy = receive_status_change(track).await;
        // Assert: draft-18 §9.1 compares no status, and the copy is passed on
        assert!(
            matches!(&second_copy, Ok(Some(Subgroup::Object(object)))
                if object.status == ObjectStatus::EndOfTrack),
            "{second_copy:?}"
        );
    }

    async fn assert_copy_arriving_in_pieces(track: PublishedTrack) {
        // Arrange: the second copy's payload is sent in two pieces, the
        // second only after the first piece has been read
        let first = object_with_extensions();
        let mut receiver = data_receiver(&track.server, &track.accepted).await;
        within(send_subgroup_object(
            &track.client,
            &track.published,
            &first,
        ))
        .await;
        let mut first_stream = within(next_subgroup(&mut receiver)).await;
        within(read_subgroup_objects(&mut first_stream)).await;
        let ForwardingPreference::Subgroup { subgroup_id } = first.forwarding else {
            unreachable!("object_with_extensions is a subgroup object");
        };
        let mut sender = within(async {
            track
                .client
                .publisher()
                .create_stream(&track.published)
                .next()
                .await
                .unwrap()
                .start(SubgroupStart {
                    group_id: first.location.group_id,
                    subgroup_id,
                    publisher_priority: first.publisher_priority,
                    ends_group: false,
                })
                .await
                .unwrap()
        })
        .await;
        let other = Bytes::from_static(b"other payload");
        let head = Object {
            payload: Bytes::new(),
            ..first.clone()
        };
        within(sender.begin_object(&head, other.len()))
            .await
            .unwrap();
        within(sender.send_payload(&other.slice(..1)))
            .await
            .unwrap();
        let mut second_stream = within(next_subgroup(&mut receiver)).await.into_parts();
        within(second_stream.receive()).await.unwrap();
        let start = within(second_stream.receive()).await;
        let first_piece = within(second_stream.receive()).await;

        // Act
        within(sender.send_payload(&other.slice(1..)))
            .await
            .unwrap();
        let last_piece = within(second_stream.receive()).await;

        // Assert: draft-14 §8.1 / draft-18 §9.1, checked once the payload is
        // complete
        assert!(
            matches!(start, Ok(Some(SubgroupPart::ObjectStart { .. }))),
            "{start:?}"
        );
        assert!(
            matches!(
                first_piece,
                Ok(Some(SubgroupPart::Payload { last: false, .. }))
            ),
            "{first_piece:?}"
        );
        assert!(
            matches!(
                last_piece,
                Err(StreamReceiveError::MalformedTrack(
                    MalformedTrackError::ConflictingDuplicate { location }
                )) if location == first.location
            ),
            "{last_piece:?}"
        );
    }

    #[tokio::test]
    async fn a_copy_arriving_in_pieces_with_another_payload_is_a_malformed_track() {
        assert_copy_arriving_in_pieces(published_track("ledger-copy-in-pieces").await).await;
    }

    #[tokio::test]
    async fn a_draft18_copy_arriving_in_pieces_with_another_payload_is_a_malformed_track() {
        assert_copy_arriving_in_pieces(
            published_draft18_track("ledger-copy-in-pieces-draft18").await,
        )
        .await;
    }

    async fn assert_datagram_copies(track: PublishedTrack) {
        // Arrange
        let first = Object {
            forwarding: ForwardingPreference::Datagram,
            ..object_with_extensions()
        };
        let other_priority = Object {
            publisher_priority: first.publisher_priority + 1,
            ..first.clone()
        };
        let mut sender = track.client.publisher().create_datagram(&track.published);
        let mut receiver = data_receiver(&track.server, &track.accepted).await;

        // Act
        sender.send_object(&first).await.unwrap();
        let delivered = within(receiver.next()).await;
        sender.send_object(&other_priority).await.unwrap();
        let repeated = within(receiver.next()).await;

        // Assert
        assert!(
            matches!(delivered, Ok(TrackData::Datagram(_))),
            "{delivered:?}"
        );
        assert_eq!(
            repeated.unwrap_err().downcast_ref::<MalformedTrackError>(),
            Some(&MalformedTrackError::ConflictingDuplicate {
                location: first.location
            })
        );
    }

    #[tokio::test]
    async fn a_datagram_repeated_with_another_priority_is_a_malformed_track() {
        assert_datagram_copies(published_track("ledger-datagram-copies").await).await;
    }

    #[tokio::test]
    async fn a_draft18_datagram_repeated_with_another_priority_is_a_malformed_track() {
        assert_datagram_copies(published_draft18_track("ledger-datagram-copies-draft18").await)
            .await;
    }

    /// The server FETCHes from the client publishing `track` and reads the
    /// first object the client answers with `answer`.
    async fn assert_fetched_copy_of_a_subscribed_object(track: PublishedTrack) {
        // Arrange
        let delivered = object_with_extensions();
        let mut receiver = data_receiver(&track.server, &track.accepted).await;
        within(send_subgroup_object(
            &track.client,
            &track.published,
            &delivered,
        ))
        .await;
        let mut stream = within(next_subgroup(&mut receiver)).await;
        within(read_subgroup_objects(&mut stream)).await;

        // Act
        let fetched = fetch_answered_with(
            &track,
            &Object {
                payload: Bytes::from_static(b"other"),
                ..delivered.clone()
            },
        )
        .await;

        // Assert
        assert_eq!(
            fetched.unwrap_err().downcast_ref::<MalformedTrackError>(),
            Some(&MalformedTrackError::ConflictingDuplicate {
                location: delivered.location
            })
        );
    }

    #[tokio::test]
    async fn a_fetched_copy_of_a_subscribed_object_with_another_payload_is_a_malformed_track() {
        assert_fetched_copy_of_a_subscribed_object(published_track("ledger-fetch-copy").await)
            .await;
    }

    #[tokio::test]
    async fn a_draft18_fetched_copy_of_a_subscribed_object_with_another_payload_is_a_malformed_track()
     {
        assert_fetched_copy_of_a_subscribed_object(
            published_draft18_track("ledger-fetch-copy-draft18").await,
        )
        .await;
    }

    /// The server side of `track` receives `live` in a datagram.
    async fn receive_datagram(track: &PublishedTrack, live: &Object) -> DataReceiver {
        let mut receiver = data_receiver(&track.server, &track.accepted).await;
        track
            .client
            .publisher()
            .create_datagram(&track.published)
            .send_object(live)
            .await
            .unwrap();
        within(receiver.next()).await.unwrap();
        receiver
    }

    #[tokio::test]
    async fn a_draft14_fetched_copy_of_a_datagram_matches_it() {
        // Arrange: draft-14 FETCH sends it in the subgroup of its Object ID
        let track = published_track("ledger-fetched-datagram").await;
        let live = datagram();
        let _receiver = receive_datagram(&track, &live).await;

        // Act
        let fetched = fetch_answered_with(&track, &live).await;

        // Assert
        assert!(
            matches!(&fetched, Ok(Fetch::Object(object))
            if object.forwarding == ForwardingPreference::Subgroup {
                subgroup_id: live.location.object_id
            }),
            "{fetched:?}"
        );
    }

    #[tokio::test]
    async fn a_draft18_fetched_copy_of_a_datagram_is_compared_with_its_preference() {
        // Arrange
        let track = published_draft18_track("ledger-fetched-datagram-draft18").await;
        let live = datagram();
        let _receiver = receive_datagram(&track, &live).await;

        // Act
        let as_datagram = fetch_answered_with(&track, &live).await;
        let in_subgroup =
            fetch_answered_with(&track, &fetched_in(&live, live.location.object_id)).await;

        // Assert
        assert!(
            matches!(&as_datagram, Ok(Fetch::Object(object))
                if object.forwarding == ForwardingPreference::Datagram),
            "{as_datagram:?}"
        );
        assert_eq!(
            in_subgroup
                .unwrap_err()
                .downcast_ref::<MalformedTrackError>(),
            Some(&MalformedTrackError::ConflictingDuplicate {
                location: live.location
            })
        );
    }
}
