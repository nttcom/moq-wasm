use tokio::sync::mpsc::UnboundedReceiver;

use crate::{
    TransportProtocol,
    modules::moqt::data_plane::{
        notification::StreamWithObject, stream::data_receiver::StreamDataReceiver,
    },
};

pub struct StreamDataReceiverFactory<T: TransportProtocol> {
    pending: Option<StreamDataReceiver<T>>,
    pub track_alias: u64,
    rest: UnboundedReceiver<StreamWithObject<T>>,
}

impl<T: TransportProtocol> StreamDataReceiverFactory<T> {
    pub(crate) fn new(
        first: StreamDataReceiver<T>,
        rest: UnboundedReceiver<StreamWithObject<T>>,
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
            Some(StreamWithObject::StreamHeader { stream, header }) => {
                Ok(StreamDataReceiver::new(stream, header).await?)
            }
            Some(StreamWithObject::Datagram(_)) => {
                anyhow::bail!("Expected StreamHeader but got Datagram")
            }
            None => anyhow::bail!("Stream channel closed"),
        }
    }
}
