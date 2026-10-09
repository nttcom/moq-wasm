use tokio::sync::mpsc::UnboundedReceiver;

use crate::modules::moqt::{
    data_plane::{
        malformed_track_error::MalformedTrackError,
        stream::stream_data_receiver::StreamDataReceiver,
    },
    runtime::dispatch::incoming_object::IncomingObject,
};

pub struct StreamDataReceiverFactory {
    pending: Option<StreamDataReceiver>,
    pub track_alias: u64,
    rest: UnboundedReceiver<IncomingObject>,
}

impl StreamDataReceiverFactory {
    pub(crate) fn new(first: StreamDataReceiver, rest: UnboundedReceiver<IncomingObject>) -> Self {
        let track_alias = first.track_alias;
        Self {
            pending: Some(first),
            track_alias,
            rest,
        }
    }

    pub async fn next(&mut self) -> anyhow::Result<StreamDataReceiver> {
        if let Some(first) = self.pending.take() {
            return Ok(first);
        }
        match self.rest.recv().await {
            Some(IncomingObject::StreamHeader { stream, header }) => {
                Ok(StreamDataReceiver::new(stream, header))
            }
            Some(IncomingObject::Datagram(_)) => Err(MalformedTrackError.into()),
            None => anyhow::bail!("Stream channel closed"),
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
    async fn a_datagram_on_a_subgroup_track_is_a_malformed_track() {
        // Arrange
        let track = published_track("stream-factory-malformed").await;
        let publisher = track.client.publisher();
        let _subgroup = open_subgroup(
            &publisher.create_stream(&track.published),
            0,
            SubgroupId::None,
        )
        .await;
        let DataReceiver::Stream(mut factory) = track
            .server
            .subscriber()
            .accept_data_receiver(&track.accepted)
            .await
            .unwrap()
        else {
            panic!("expected a subgroup stream receiver");
        };
        factory.next().await.unwrap();
        publisher
            .create_datagram(&track.published)
            .send(object_datagram(track.published.track_alias()))
            .await
            .unwrap();

        // Act
        let result = tokio::time::timeout(HANDSHAKE_TIMEOUT, factory.next())
            .await
            .unwrap();

        // Assert
        assert!(result.unwrap_err().is::<MalformedTrackError>());
    }
}
