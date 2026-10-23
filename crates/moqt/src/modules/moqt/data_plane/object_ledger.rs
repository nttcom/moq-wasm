use std::{
    collections::{HashMap, hash_map::Entry},
    hash::{BuildHasher, RandomState},
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
};

/// The objects of one track a session has received, kept while a
/// subscription or FETCH of the track is read. No entry is ever removed, so
/// the ledger grows by one entry per object for that whole time. An object
/// that arrives again with another forwarding preference, subgroup, priority
/// or payload, or whose status moves between Normal, End of Group and End of
/// Track, makes the track malformed (draft-14 §8.1, draft-18 §9.1). Its
/// extension headers may change, and so may its status to or from Does Not
/// Exist.
pub(crate) struct ObjectLedger {
    hasher: RandomState,
    objects: std::sync::Mutex<HashMap<Location, Recorded>>,
}

/// What a duplicate is compared on; the payload is kept as a hash so the
/// ledger stays small.
struct Recorded {
    forwarding: ForwardingPreference,
    publisher_priority: u8,
    status: ObjectStatus,
    payload_hash: u64,
}

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

    /// Records `object`; fails when an earlier copy of it differs.
    pub(crate) fn check(&self, object: &Object) -> Result<(), MalformedTrackError> {
        let received = Recorded {
            forwarding: object.forwarding,
            publisher_priority: object.publisher_priority,
            status: object.status,
            payload_hash: self.hasher.hash_one(&object.payload),
        };
        let mut objects = self
            .objects
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match objects.entry(object.location) {
            Entry::Vacant(slot) => {
                slot.insert(received);
            }
            Entry::Occupied(mut recorded) => {
                if recorded.get().conflicts_with(&received) {
                    return Err(MalformedTrackError::ConflictingDuplicate {
                        location: object.location,
                    });
                }
                if recorded.get().status == ObjectStatus::DoesNotExist {
                    recorded.insert(received);
                }
            }
        }
        Ok(())
    }
}

impl Recorded {
    fn conflicts_with(&self, other: &Self) -> bool {
        if self.forwarding != other.forwarding
            || self.publisher_priority != other.publisher_priority
        {
            return true;
        }
        if self.status == ObjectStatus::DoesNotExist || other.status == ObjectStatus::DoesNotExist {
            return false;
        }
        self.status != other.status || self.payload_hash != other.payload_hash
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::ObjectLedger;
    use crate::{
        SessionEvent,
        fetch::{Fetch, FetchOption},
        modules::test_support::{
            PublishedTrack, data_receiver, location, next_subgroup, object_with_extensions,
            published_draft18_track, published_track, read_subgroup_objects, send_subgroup_object,
            within,
        },
        object::{
            DataReceiver, ExtensionHeaders, ForwardingPreference, KeyValuePair,
            MalformedTrackError, Object, ObjectStatus, StreamReceiveError, Subgroup, TrackData,
            VariantType,
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

    fn check_both(first: &Object, second: &Object) -> Result<(), MalformedTrackError> {
        let ledger = ObjectLedger::new();
        ledger.check(first).unwrap();
        ledger.check(second)
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
    fn a_status_moving_between_normal_end_of_group_and_end_of_track_is_a_conflict() {
        let statuses = [
            ObjectStatus::Normal,
            ObjectStatus::EndOfGroup,
            ObjectStatus::EndOfTrack,
        ];
        for first in statuses {
            for second in statuses.into_iter().filter(|second| *second != first) {
                // Arrange
                let (first, second) = (status_object(first), status_object(second));
                // Act / Assert
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
    fn a_status_moving_to_or_from_does_not_exist_passes() {
        // Arrange
        let normal = object_with_extensions();
        let missing = status_object(ObjectStatus::DoesNotExist);
        // Act / Assert: both orders
        assert_eq!(check_both(&normal, &missing), Ok(()));
        assert_eq!(check_both(&missing, &normal), Ok(()));
    }

    #[test]
    fn an_object_found_missing_is_still_compared_with_its_first_copy() {
        // Arrange
        let ledger = ObjectLedger::new();
        let normal = object_with_extensions();
        ledger.check(&normal).unwrap();
        ledger
            .check(&status_object(ObjectStatus::DoesNotExist))
            .unwrap();
        let other = Object {
            payload: Bytes::from_static(b"other"),
            ..normal.clone()
        };
        // Act / Assert
        assert_eq!(ledger.check(&other), conflict(&normal));
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
    async fn fetch_answered_with(track: &PublishedTrack, answer: &Object) -> anyhow::Result<Fetch> {
        let mut subscriber = track.server.subscriber();
        let end = location(answer.location.group_id + 1, 0);
        let (handle, sender) = within(async {
            tokio::join!(
                subscriber.fetch(
                    "ns".into(),
                    "track".into(),
                    location(0, 0),
                    end,
                    FetchOption::default(),
                ),
                async {
                    let SessionEvent::Fetch(handler) = track.client.receive_event().await.unwrap()
                    else {
                        panic!("expected FETCH from the server");
                    };
                    handler.ok(false, end).await.unwrap();
                    track
                        .client
                        .publisher()
                        .create_fetch_stream(handler.request_id)
                        .await
                        .unwrap()
                }
            )
        })
        .await;
        sender.send_object(answer).await.unwrap();
        let mut receiver = within(subscriber.accept_fetch_receiver(&handle.unwrap()))
            .await
            .unwrap();
        within(receiver.receive()).await
    }

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
}
