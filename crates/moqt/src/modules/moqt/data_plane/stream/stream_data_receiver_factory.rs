use tokio::sync::mpsc::UnboundedReceiver;

use crate::modules::moqt::{
    data_plane::stream::stream_data_receiver::StreamDataReceiver,
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
            Some(IncomingObject::Datagram(_)) => {
                anyhow::bail!("Expected StreamHeader but got Datagram")
            }
            None => anyhow::bail!("Stream channel closed"),
        }
    }
}
