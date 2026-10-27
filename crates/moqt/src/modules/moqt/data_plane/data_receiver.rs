use std::sync::Arc;

use tokio::sync::mpsc::UnboundedReceiver;

use crate::modules::moqt::{
    data_plane::{
        immutable_properties,
        malformed_track_error::MalformedTrackError,
        object::canonical_object::Object,
        object_ledger::{CopySource, ObjectLedger},
        stream::stream_data_receiver::StreamDataReceiver,
    },
    runtime::dispatch::incoming_object::IncomingObject,
    version::Version,
};

/// What a subscription delivers next.
#[derive(Debug)]
pub enum TrackData {
    /// A subgroup stream; its header and objects are read from the receiver.
    Subgroup(StreamDataReceiver),
    Datagram(Object),
}

/// The subgroup streams and datagrams of one subscription, in arrival order.
#[derive(Debug)]
pub struct DataReceiver {
    pub track_alias: u64,
    receiver: UnboundedReceiver<IncomingObject>,
    /// The draft of the connection the subscription was accepted on.
    version: Version,
    first_is_datagram: Option<bool>,
    ledger: Arc<ObjectLedger>,
}

impl DataReceiver {
    pub(crate) fn new(
        track_alias: u64,
        receiver: UnboundedReceiver<IncomingObject>,
        version: Version,
        ledger: Arc<ObjectLedger>,
    ) -> Self {
        Self {
            track_alias,
            receiver,
            version,
            first_is_datagram: None,
            ledger,
        }
    }

    /// Whether a duplicate object whose status differs from an earlier copy
    /// makes the track malformed on this session: draft-14 §8.1 compares the
    /// status, draft-18 §9.1 does not.
    pub fn compares_duplicate_status(&self) -> bool {
        self.version.compares_duplicate_status()
    }

    /// Waits for the next subgroup stream or datagram. Fails with a
    /// `MalformedTrackError` when a draft-14 track mixes the two (§2.5) or a
    /// datagram conflicts with an earlier copy of its object, and with another
    /// error once the subscription has ended. A subgroup stream's objects are
    /// checked as they are read from it.
    pub async fn next(&mut self) -> anyhow::Result<TrackData> {
        let Some(incoming) = self.receiver.recv().await else {
            anyhow::bail!("the subscription ended");
        };
        let is_datagram = matches!(incoming, IncomingObject::Datagram(_));
        if !self.version.mixes_forwarding_preferences()
            && *self.first_is_datagram.get_or_insert(is_datagram) != is_datagram
        {
            return Err(MalformedTrackError::MixedForwardingPreference.into());
        }
        Ok(match incoming {
            IncomingObject::Subgroup(mut receiver) => {
                receiver.check_with(self.ledger.clone());
                TrackData::Subgroup(receiver)
            }
            IncomingObject::Datagram(datagram) => {
                immutable_properties::check(self.version, &datagram.object)?;
                self.ledger
                    .check(&datagram.object, CopySource::Subscription(self.version))?;
                TrackData::Datagram(datagram.object)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        Version,
        modules::test_support::{
            HANDSHAKE_TIMEOUT, PublishedTrack, data_receiver, datagram_object, next_datagram,
            next_subgroup, object_with_extensions, open_raw_subgroup, published_track,
            published_track_over,
        },
        object::{
            DataReceiver, ForwardingPreference, MalformedTrackError, ObjectStatus,
            SubgroupObjectSender, SubgroupStart,
        },
        wire::SubgroupId,
    };

    async fn send_datagram(track: &PublishedTrack, group_id: u64) {
        let mut object = datagram_object();
        object.location.group_id = group_id;
        track
            .client
            .publisher()
            .create_datagram(&track.published)
            .send_object(&object)
            .await
            .unwrap();
    }

    async fn open_stream(track: &PublishedTrack, group_id: u64) -> SubgroupObjectSender {
        track
            .client
            .publisher()
            .create_stream(&track.published)
            .next()
            .await
            .unwrap()
            .start(SubgroupStart {
                group_id,
                subgroup_id: 0,
                publisher_priority: 128,
                ends_group: false,
            })
            .await
            .unwrap()
    }

    async fn next_group(receiver: &mut DataReceiver) -> (bool, u64) {
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, receiver.next())
            .await
            .unwrap()
            .unwrap()
        {
            super::TrackData::Subgroup(stream) => (false, stream.group_id()),
            super::TrackData::Datagram(object) => (true, object.location.group_id),
        }
    }

    async fn assert_payload_datagram_round_trip(name: &str, ends_group: bool) {
        // Arrange
        let track = published_track(name).await;
        let publisher = track.client.publisher();
        let mut sender = publisher.create_datagram(&track.published);
        let mut object = object_with_extensions();
        object.forwarding = ForwardingPreference::Datagram;
        object.ends_group = ends_group;
        let mut receiver = data_receiver(&track.server, &track.accepted).await;

        // Act
        let received = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            sender.send_object(&object).await.unwrap();
            next_datagram(&mut receiver).await
        })
        .await
        .unwrap();

        // Assert
        assert_eq!(received, object);
    }

    #[tokio::test]
    async fn payload_datagrams_preserve_object_properties() {
        assert_payload_datagram_round_trip("datagram-object-properties", false).await;
    }

    #[tokio::test]
    async fn payload_datagrams_preserve_the_end_of_group_marker() {
        assert_payload_datagram_round_trip("datagram-end-of-group", true).await;
    }

    #[tokio::test]
    async fn empty_datagrams_preserve_normal_and_non_normal_object_statuses() {
        // Arrange
        let track = published_track("datagram-empty-object-statuses").await;
        let publisher = track.client.publisher();
        let mut sender = publisher.create_datagram(&track.published);
        let mut object = object_with_extensions();
        object.forwarding = ForwardingPreference::Datagram;
        object.payload = Default::default();
        let mut absent = object.clone();
        absent.location.object_id += 1;
        absent.status = ObjectStatus::DoesNotExist;
        absent.extension_headers = Default::default();
        let mut next_normal = object.clone();
        next_normal.location.object_id += 2;
        let mut end_of_group = next_normal.clone();
        end_of_group.location.object_id += 1;
        end_of_group.status = ObjectStatus::EndOfGroup;
        let mut end_of_track = object.clone();
        end_of_track.location.group_id += 1;
        end_of_track.location.object_id = 0;
        end_of_track.status = ObjectStatus::EndOfTrack;
        let expected = [object, absent, next_normal, end_of_group, end_of_track];
        let mut receiver = data_receiver(&track.server, &track.accepted).await;

        // Act
        let received = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            for object in &expected {
                sender.send_object(object).await.unwrap();
            }
            let mut received = Vec::new();
            for _ in &expected {
                received.push(next_datagram(&mut receiver).await);
            }
            received
        })
        .await
        .unwrap();

        // Assert
        assert_eq!(received, expected);
    }

    #[tokio::test]
    async fn a_draft14_subgroup_stream_after_a_datagram_is_a_malformed_track() {
        // Arrange
        let track = published_track("data-receiver-datagram-then-stream").await;
        let publisher = track.client.publisher();
        let mut receiver = data_receiver(&track.server, &track.accepted).await;
        publisher
            .create_datagram(&track.published)
            .send_object(&datagram_object())
            .await
            .unwrap();
        next_datagram(&mut receiver).await;
        let _subgroup = open_raw_subgroup(
            &publisher.create_stream(&track.published),
            0,
            SubgroupId::None,
        )
        .await;

        // Act
        let result = tokio::time::timeout(HANDSHAKE_TIMEOUT, receiver.next())
            .await
            .unwrap();

        // Assert
        assert_eq!(
            result.unwrap_err().downcast_ref::<MalformedTrackError>(),
            Some(&MalformedTrackError::MixedForwardingPreference)
        );
    }

    #[tokio::test]
    async fn a_draft14_datagram_after_a_subgroup_stream_is_a_malformed_track() {
        // Arrange
        let track = published_track("data-receiver-stream-then-datagram").await;
        let publisher = track.client.publisher();
        let mut receiver = data_receiver(&track.server, &track.accepted).await;
        let _subgroup = open_raw_subgroup(
            &publisher.create_stream(&track.published),
            0,
            SubgroupId::None,
        )
        .await;
        next_subgroup(&mut receiver).await;
        publisher
            .create_datagram(&track.published)
            .send_object(&datagram_object())
            .await
            .unwrap();

        // Act
        let result = tokio::time::timeout(HANDSHAKE_TIMEOUT, receiver.next())
            .await
            .unwrap();

        // Assert
        assert_eq!(
            result.unwrap_err().downcast_ref::<MalformedTrackError>(),
            Some(&MalformedTrackError::MixedForwardingPreference)
        );
    }

    #[tokio::test]
    async fn a_draft18_track_delivers_a_subgroup_stream_after_a_datagram() {
        // Arrange
        let track = published_track_over(
            "data-receiver-draft18-datagram-then-stream",
            Version::Draft18,
        )
        .await;
        let mut receiver = data_receiver(&track.server, &track.accepted).await;
        send_datagram(&track, 0).await;
        let first = next_group(&mut receiver).await;

        // Act
        let _stream = open_stream(&track, 1).await;
        let second = next_group(&mut receiver).await;
        send_datagram(&track, 2).await;
        let third = next_group(&mut receiver).await;

        // Assert: true marks a datagram; the subscription goes on after the mix
        assert_eq!([first, second, third], [(true, 0), (false, 1), (true, 2)]);
    }

    #[tokio::test]
    async fn a_draft18_track_delivers_a_datagram_after_a_subgroup_stream() {
        // Arrange
        let track = published_track_over(
            "data-receiver-draft18-stream-then-datagram",
            Version::Draft18,
        )
        .await;
        let mut receiver = data_receiver(&track.server, &track.accepted).await;
        let _first_stream = open_stream(&track, 0).await;
        let first = next_group(&mut receiver).await;

        // Act
        send_datagram(&track, 1).await;
        let second = next_group(&mut receiver).await;
        let _third_stream = open_stream(&track, 2).await;
        let third = next_group(&mut receiver).await;

        // Assert: true marks a datagram; the subscription goes on after the mix
        assert_eq!([first, second, third], [(false, 0), (true, 1), (false, 2)]);
    }

    #[tokio::test]
    async fn a_draft14_subscription_compares_the_status_of_duplicates() {
        // Arrange
        let track = published_track("data-receiver-draft14-duplicate-status").await;
        // Act
        let receiver = data_receiver(&track.server, &track.accepted).await;
        // Assert: draft-14 §8.1
        assert!(receiver.compares_duplicate_status());
    }

    #[tokio::test]
    async fn a_draft18_subscription_does_not_compare_the_status_of_duplicates() {
        // Arrange
        let track =
            published_track_over("data-receiver-draft18-duplicate-status", Version::Draft18).await;
        // Act
        let receiver = data_receiver(&track.server, &track.accepted).await;
        // Assert: draft-18 §9.1 compares only the forwarding preference,
        // subgroup, priority and payload
        assert!(!receiver.compares_duplicate_status());
    }
}
