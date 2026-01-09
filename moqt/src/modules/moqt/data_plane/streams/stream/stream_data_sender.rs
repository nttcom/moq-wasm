use std::sync::Arc;

use crate::{
    TransportProtocol,
    modules::{
        moqt::{
            control_plane::models::session_context::SessionContext,
            data_plane::{
                object::data_object::DataObject,
                streams::{stream::stream_sender::StreamSender, stream_type::SendStreamType},
            },
        },
        transport::transport_connection::TransportConnection,
    },
};

pub struct StreamDataSender<T: TransportProtocol> {
    session_context: Arc<SessionContext<T>>,
    stream_sender: StreamSender<T>,
}

#[async_trait::async_trait]
impl<T: TransportProtocol> SendStreamType for StreamDataSender<T> {
    fn is_datagram(&self) -> bool {
        false
    }

    async fn send(&mut self, data: DataObject) -> anyhow::Result<()> {
        match data {
            DataObject::SubgroupHeader(header) => {
                let new_stream_sender = Self::create_sender(&self.session_context).await?;
                self.stream_sender = new_stream_sender;

                let bytes = header.encode()?;
                self.stream_sender.send(&bytes).await
            }

            DataObject::SubgroupObject(subgroup) => {
                let bytes = subgroup.encode();
                self.stream_sender.send(&bytes).await
            }

            _ => unreachable!("StreamDataSender can only send SubgroupHeader and SubgroupObject"),
        }
    }
}

impl<T: TransportProtocol> StreamDataSender<T> {
    async fn create_sender(
        session_context: &Arc<SessionContext<T>>,
    ) -> anyhow::Result<StreamSender<T>> {
        let stream = session_context.transport_connection.open_uni().await?;
        let stream_sender = StreamSender::new(stream);
        Ok(stream_sender)
    }

    pub(crate) async fn new(session_context: Arc<SessionContext<T>>) -> anyhow::Result<Self> {
        let stream_sender = Self::create_sender(&session_context).await?;
        Ok(Self {
            session_context,
            stream_sender,
        })
    }
}
