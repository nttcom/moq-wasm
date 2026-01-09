use crate::{
    TransportProtocol,
    modules::moqt::{
        control_plane::threads::enums::StreamWithObject,
        data_plane::{
            object::{data_object::DataObject, subgroup::SubgroupObjectField},
            streams::{stream::stream_receiver::StreamReceiver, stream_type::ReceiveStreamType},
        },
    },
};

#[derive(Debug)]
pub struct StreamDataReceiver<T: TransportProtocol> {
    stream_receiver: StreamReceiver<T>,
    receiver: tokio::sync::mpsc::UnboundedReceiver<StreamWithObject<T>>,
}

#[async_trait::async_trait]
impl<T: TransportProtocol> ReceiveStreamType<T> for StreamDataReceiver<T> {
    fn is_datagram(&self) -> bool {
        false
    }

    async fn receive(&mut self) -> anyhow::Result<DataObject> {
        tokio::select! {
            data = self.stream_receiver.receive() => {
                let data = data?;
                let subgroup = SubgroupObjectField::decode(data).ok_or_else(|| anyhow::anyhow!("Failed to decode subgroup object"))?;
                Ok(DataObject::SubgroupObject(subgroup))
            },
            data = self.receiver.recv() => {
                let data = data.ok_or_else(|| anyhow::anyhow!("Failed to receive stream"))?;
                match data {
                    StreamWithObject::StreamHeader{stream, header } => {
                        self.stream_receiver = stream;
                        Ok(DataObject::SubgroupHeader(header))
                    },
                    _ => Err(anyhow::anyhow!("Received unexpected stream type")),
                }
            }
        }
    }
}

impl<T: TransportProtocol> StreamDataReceiver<T> {
    pub(crate) async fn new(
        receiver: tokio::sync::mpsc::UnboundedReceiver<StreamWithObject<T>>,
        stream: StreamReceiver<T>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            stream_receiver: stream,
            receiver,
        })
    }
}
