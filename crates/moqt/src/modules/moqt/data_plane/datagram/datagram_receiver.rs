use anyhow::bail;

use crate::modules::moqt::data_plane::{
    malformed_track_error::MalformedTrackError, object::object_datagram::ObjectDatagram,
};
use crate::modules::moqt::runtime::dispatch::incoming_object::IncomingObject;

#[derive(Debug)]
pub struct DatagramReceiver {
    pub track_alias: u64,
    receiver: tokio::sync::mpsc::UnboundedReceiver<IncomingObject>,
    first_object_datagram: Option<ObjectDatagram>,
}

impl DatagramReceiver {
    pub(crate) async fn new(
        first_object_datagram: ObjectDatagram,
        receiver: tokio::sync::mpsc::UnboundedReceiver<IncomingObject>,
    ) -> Self {
        let track_alias = first_object_datagram.track_alias;
        Self {
            track_alias,
            first_object_datagram: Some(first_object_datagram),
            receiver,
        }
    }

    pub async fn receive(&mut self) -> anyhow::Result<ObjectDatagram> {
        if let Some(object_datagram) = self.first_object_datagram.take() {
            return Ok(object_datagram);
        }
        let result = match self.receiver.recv().await {
            Some(object) => object,
            None => bail!("Sender dropped."),
        };
        match result {
            IncomingObject::Datagram(datagram) => Ok(datagram),
            IncomingObject::StreamHeader { .. } => Err(MalformedTrackError.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        DataReceiver, MalformedTrackError, SubgroupId,
        modules::test_support::{
            HANDSHAKE_TIMEOUT, object_datagram, open_subgroup, published_track,
        },
    };

    #[tokio::test]
    async fn a_subgroup_stream_on_a_datagram_track_is_a_malformed_track() {
        // Arrange
        let track = published_track("datagram-receiver-malformed").await;
        let publisher = track.client.publisher();
        publisher
            .create_datagram(&track.published)
            .send(object_datagram(track.published.track_alias()))
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
