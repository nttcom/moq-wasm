use crate::{
    TransportProtocol,
    modules::moqt::{
        control_plane::threads::enums::StreamWithObject,
        data_plane::{
            object::subgroup::{SubgroupHeader, SubgroupHeaderType, SubgroupObjectField},
            streams::stream::stream_receiver::StreamReceiver,
        },
    },
};

#[derive(Debug)]
pub enum Subgroup {
    Header(SubgroupHeader),
    Object(SubgroupObjectField),
}

#[derive(Debug)]
pub struct StreamDataReceiver<T: TransportProtocol> {
    stream_receiver: StreamReceiver<T>,
    receiver: tokio::sync::mpsc::UnboundedReceiver<StreamWithObject<T>>,
    pub track_alias: u64,
    first_subgroup_header: Option<SubgroupHeader>,
    subgroup_header_type: SubgroupHeaderType,
}

impl<T: TransportProtocol> StreamDataReceiver<T> {
    pub(crate) async fn new(
        receiver: tokio::sync::mpsc::UnboundedReceiver<StreamWithObject<T>>,
        stream: StreamReceiver<T>,
        subgroup_header: SubgroupHeader,
    ) -> anyhow::Result<Self> {
        let track_alias = subgroup_header.track_alias;
        let subgroup_header_type = subgroup_header.message_type;
        Ok(Self {
            stream_receiver: stream,
            receiver,
            track_alias,
            first_subgroup_header: Some(subgroup_header),
            subgroup_header_type,
        })
    }

    pub async fn receive(&mut self) -> anyhow::Result<Subgroup> {
        if let Some(subgroup_header) = self.first_subgroup_header.take() {
            return Ok(Subgroup::Header(subgroup_header));
        }

        let data = self.stream_receiver.receive().await;
        if let Ok(data) = data {
            let subgroup = SubgroupObjectField::decode(self.subgroup_header_type, data)
                .ok_or_else(|| anyhow::anyhow!("Failed to decode subgroup object"))?;
            Ok(Subgroup::Object(subgroup))
        } else {
            tracing::debug!("No data received, checking for new stream...");
            let new_stream = self
                .receiver
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("Failed to receive stream"))?;
            match new_stream {
                StreamWithObject::StreamHeader { stream, header } => {
                    self.stream_receiver = stream;
                    self.subgroup_header_type = header.message_type;
                    Ok(Subgroup::Header(header))
                }
                _ => Err(anyhow::anyhow!("Received unexpected stream type")),
            }
        }
    }
}
