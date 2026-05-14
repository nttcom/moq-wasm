use tokio::sync::mpsc::UnboundedReceiver;

use crate::{
    TransportProtocol,
    modules::moqt::{
        data_plane::streams::stream::stream_data_receiver::StreamDataReceiver,
        runtime::dispatch::incoming_track_data::IncomingTrackData,
    },
};

pub struct StreamDataReceiverFactory<T: TransportProtocol> {
    pending: Option<StreamDataReceiver<T>>,
    pub track_alias: u64,
    rest: UnboundedReceiver<IncomingTrackData<T>>,
}

impl<T: TransportProtocol> StreamDataReceiverFactory<T> {
    pub(crate) fn new(
        first: StreamDataReceiver<T>,
        rest: UnboundedReceiver<IncomingTrackData<T>>,
    ) -> Self {
        let track_alias = first.track_alias;
        Self {
            pending: Some(first),
            track_alias,
            rest,
        }
    }

    pub async fn next(&mut self) -> anyhow::Result<StreamDataReceiver<T>> {
        if let Some(first) = self.pending.take() {
            return Ok(first);
        }
        match self.rest.recv().await {
            Some(IncomingTrackData::StreamHeader { stream, header }) => {
                Ok(StreamDataReceiver::new(stream, header).await?)
            }
            Some(IncomingTrackData::Datagram(_)) => {
                anyhow::bail!("Expected StreamHeader but got Datagram")
            }
            None => anyhow::bail!("Stream channel closed"),
        }
    }
}
