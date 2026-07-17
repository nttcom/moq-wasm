use crate::{
    TransportProtocol,
    modules::moqt::data_plane::{
        codec::uni_stream_decoder::UniStreamData,
        object::subgroup::{SubgroupHeader, SubgroupObjectField},
        stream::stream_receiver::{StreamReceiveError, UniStreamReceiver},
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

    /// Returns `Ok(None)` when the peer finished the stream normally (FIN).
    /// Transport-level closure and decode failures are reported as distinct
    /// `StreamReceiveError` variants so callers can react per cause.
    pub async fn receive(&mut self) -> Result<Option<Subgroup>, StreamReceiveError> {
        if let Some(subgroup_header) = self.first_subgroup_header.take() {
            return Ok(Some(Subgroup::Header(subgroup_header)));
        }

        match self.stream_receiver.receive().await {
            Ok(Some(UniStreamData::Subgroup(subgroup))) => Ok(Some(subgroup)),
            Ok(Some(UniStreamData::Fetch(_))) => {
                unreachable!("Unexpected fetch data in subgroup stream")
            }
            Ok(None) => {
                tracing::debug!("Stream data ended");
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}
