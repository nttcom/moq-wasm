use tokio::sync::mpsc::UnboundedReceiver;

use crate::modules::moqt::{
    data_plane::{
        malformed_track_error::MalformedTrackError, object::canonical_object::Object,
        stream::stream_data_receiver::StreamDataReceiver,
    },
    runtime::dispatch::incoming_object::IncomingObject,
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
    first_is_datagram: Option<bool>,
}

impl DataReceiver {
    pub(crate) fn new(track_alias: u64, receiver: UnboundedReceiver<IncomingObject>) -> Self {
        Self {
            track_alias,
            receiver,
            first_is_datagram: None,
        }
    }

    /// Waits for the next subgroup stream or datagram of the track. Fails
    /// with a `MalformedTrackError` when the track mixes the two (draft-14
    /// §2.5), and with another error once the subscription has ended.
    pub async fn next(&mut self) -> anyhow::Result<TrackData> {
        let Some(incoming) = self.receiver.recv().await else {
            anyhow::bail!("the subscription ended");
        };
        let is_datagram = matches!(incoming, IncomingObject::Datagram(_));
        if *self.first_is_datagram.get_or_insert(is_datagram) != is_datagram {
            return Err(MalformedTrackError.into());
        }
        Ok(match incoming {
            IncomingObject::Subgroup(receiver) => TrackData::Subgroup(receiver),
            IncomingObject::Datagram(datagram) => TrackData::Datagram(datagram.object),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        modules::test_support::{
            HANDSHAKE_TIMEOUT, data_receiver, datagram_object, next_datagram, next_subgroup,
            object_with_extensions, open_subgroup, published_track,
        },
        object::{ForwardingPreference, MalformedTrackError, ObjectStatus},
        wire::SubgroupId,
    };

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
        let _subgroup = open_subgroup(
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
        assert!(result.unwrap_err().is::<MalformedTrackError>());
    }

    #[tokio::test]
    async fn a_draft14_datagram_after_a_subgroup_stream_is_a_malformed_track() {
        // Arrange
        let track = published_track("data-receiver-stream-then-datagram").await;
        let publisher = track.client.publisher();
        let mut receiver = data_receiver(&track.server, &track.accepted).await;
        let _subgroup = open_subgroup(
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
        assert!(result.unwrap_err().is::<MalformedTrackError>());
    }
}
