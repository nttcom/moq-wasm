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
            HANDSHAKE_TIMEOUT, datagram_object, open_subgroup, published_track,
        },
        object::{DataReceiver, MalformedTrackError},
        wire::SubgroupId,
    };

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
