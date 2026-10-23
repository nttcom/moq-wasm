use anyhow::bail;

use crate::modules::moqt::data_plane::{
    malformed_track_error::MalformedTrackError, object::canonical_object::Object,
};
use crate::modules::moqt::runtime::dispatch::incoming_object::IncomingObject;

#[derive(Debug)]
pub struct DatagramReceiver {
    pub track_alias: u64,
    receiver: tokio::sync::mpsc::UnboundedReceiver<IncomingObject>,
    first_object: Option<Object>,
}

impl DatagramReceiver {
    pub(crate) fn new(
        track_alias: u64,
        first_object: Object,
        receiver: tokio::sync::mpsc::UnboundedReceiver<IncomingObject>,
    ) -> Self {
        Self {
            track_alias,
            first_object: Some(first_object),
            receiver,
        }
    }

    /// Returns the next object of the track. A subgroup stream on the track
    /// is a `MalformedTrackError` (draft-14 §2.5).
    pub async fn receive(&mut self) -> anyhow::Result<Object> {
        if let Some(object) = self.first_object.take() {
            return Ok(object);
        }
        let result = match self.receiver.recv().await {
            Some(object) => object,
            None => bail!("Sender dropped."),
        };
        match result {
            IncomingObject::Datagram(object) => Ok(object),
            IncomingObject::Subgroup(_) => Err(MalformedTrackError.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        modules::test_support::{
            HANDSHAKE_TIMEOUT, datagram_object, object_with_extensions, open_subgroup,
            published_track,
        },
        object::{DataReceiver, ForwardingPreference, MalformedTrackError, ObjectStatus},
        wire::SubgroupId,
    };

    #[tokio::test]
    async fn payload_datagrams_preserve_object_properties() {
        // Arrange
        let track = published_track("datagram-object-properties").await;
        let publisher = track.client.publisher();
        let mut sender = publisher.create_datagram(&track.published);
        let mut object = object_with_extensions();
        object.forwarding = ForwardingPreference::Datagram;

        // Act
        let received = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            sender.send_object(&object).await.unwrap();
            let DataReceiver::Datagram(mut receiver) = track
                .server
                .subscriber()
                .accept_data_receiver(&track.accepted)
                .await
                .unwrap()
            else {
                panic!("expected a datagram receiver");
            };

            receiver.receive().await.unwrap()
        })
        .await
        .unwrap();

        // Assert
        assert_eq!(received, object);
    }

    #[tokio::test]
    async fn payload_datagrams_preserve_the_end_of_group_marker() {
        // Arrange
        let track = published_track("datagram-end-of-group").await;
        let publisher = track.client.publisher();
        let mut sender = publisher.create_datagram(&track.published);
        let mut object = object_with_extensions();
        object.forwarding = ForwardingPreference::Datagram;
        object.ends_group = true;

        // Act
        let received = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            sender.send_object(&object).await.unwrap();
            let DataReceiver::Datagram(mut receiver) = track
                .server
                .subscriber()
                .accept_data_receiver(&track.accepted)
                .await
                .unwrap()
            else {
                panic!("expected a datagram receiver");
            };
            receiver.receive().await.unwrap()
        })
        .await
        .unwrap();

        // Assert
        assert_eq!(received, object);
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

        // Act
        let received = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            for object in &expected {
                sender.send_object(object).await.unwrap();
            }
            let DataReceiver::Datagram(mut receiver) = track
                .server
                .subscriber()
                .accept_data_receiver(&track.accepted)
                .await
                .unwrap()
            else {
                panic!("expected a datagram receiver");
            };

            let mut received = Vec::new();
            for _ in &expected {
                received.push(receiver.receive().await.unwrap());
            }
            received
        })
        .await
        .unwrap();

        // Assert
        assert_eq!(received, expected);
    }

    #[tokio::test]
    async fn a_subgroup_stream_on_a_datagram_track_is_a_malformed_track() {
        // Arrange
        let track = published_track("datagram-receiver-malformed").await;
        let publisher = track.client.publisher();
        publisher
            .create_datagram(&track.published)
            .send_object(&datagram_object())
            .await
            .unwrap();
        let DataReceiver::Datagram(mut receiver) = track
            .server
            .subscriber()
            .accept_data_receiver(&track.accepted)
            .await
            .unwrap()
        else {
            panic!("expected a datagram receiver");
        };
        receiver.receive().await.unwrap();
        let _subgroup = open_subgroup(
            &publisher.create_stream(&track.published),
            0,
            SubgroupId::None,
        )
        .await;

        // Act
        let result = tokio::time::timeout(HANDSHAKE_TIMEOUT, receiver.receive())
            .await
            .unwrap();

        // Assert
        assert!(result.unwrap_err().is::<MalformedTrackError>());
    }
}
