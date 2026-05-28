use crate::{
    TransportProtocol,
    modules::moqt::data_plane::{
        codec::uni_stream_decoder::UniStreamData,
        object::fetch::{FetchHeader, FetchObjectField},
        stream::stream_receiver::UniStreamReceiver,
    },
};

#[derive(Debug)]
pub enum Fetch {
    Header(FetchHeader),
    Object(FetchObjectField),
    End,
}

#[derive(Debug)]
pub struct FetchDataReceiver<T: TransportProtocol> {
    stream_receiver: UniStreamReceiver<T>,
    pub request_id: u64,
    first_fetch_header: Option<FetchHeader>,
}

impl<T: TransportProtocol> FetchDataReceiver<T> {
    pub(crate) fn new(stream: UniStreamReceiver<T>, fetch_header: FetchHeader) -> Self {
        let request_id = fetch_header.request_id;
        Self {
            stream_receiver: stream,
            request_id,
            first_fetch_header: Some(fetch_header),
        }
    }

    pub async fn receive(&mut self) -> anyhow::Result<Fetch> {
        if let Some(fetch_header) = self.first_fetch_header.take() {
            return Ok(Fetch::Header(fetch_header));
        }

        match self.stream_receiver.receive().await {
            Ok(Some(UniStreamData::Fetch(fetch))) => Ok(fetch),
            Ok(Some(UniStreamData::Subgroup(_))) => {
                unreachable!("Unexpected subgroup data in fetch stream")
            }
            Ok(None) => Ok(Fetch::End),
            Err(e) => {
                tracing::error!(?e, "Failed to receive data from fetch stream");
                anyhow::bail!("Failed to receive data from fetch stream")
            }
        }
    }
}
