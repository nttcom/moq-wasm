use crate::{
    TransportProtocol,
    modules::moqt::data_plane::{
        object::subgroup::{SubgroupHeader, SubgroupObjectField},
        stream::receiver::UniStreamReceiver,
    },
};

#[derive(Debug)]
pub enum Subgroup {
    Header(SubgroupHeader),
    Object(SubgroupObjectField),
}

#[derive(Debug)]
pub struct StreamDataReceiver<T: TransportProtocol> {
    stream_receiver: UniStreamReceiver<T>,
    pub track_alias: u64,
    first_subgroup_header: Option<SubgroupHeader>,
}

impl<T: TransportProtocol> StreamDataReceiver<T> {
    pub(crate) async fn new(
        stream: UniStreamReceiver<T>,
        subgroup_header: SubgroupHeader,
    ) -> anyhow::Result<Self> {
        let track_alias = subgroup_header.track_alias;
        Ok(Self {
            stream_receiver: stream,
            track_alias,
            first_subgroup_header: Some(subgroup_header),
        })
    }

    pub async fn receive(&mut self) -> anyhow::Result<Subgroup> {
        if let Some(subgroup_header) = self.first_subgroup_header.take() {
            return Ok(Subgroup::Header(subgroup_header));
        }

        match self.stream_receiver.receive().await {
            Some(Ok(subgroup)) => Ok(subgroup),
            _ => {
                tracing::error!("Failed to receive data from stream");
                anyhow::bail!("Failed to receive data from stream")
            }
        }
    }
}
