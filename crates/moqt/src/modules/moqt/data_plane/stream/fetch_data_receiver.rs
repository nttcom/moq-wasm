use crate::modules::moqt::data_plane::{
    codec::{fetch_decoder::FetchFrame, uni_stream_decoder::UniStreamData},
    object::{canonical_object::Object, fetch::FetchHeader},
    stream::stream_receiver::{StreamReceiveError, UniStreamReceiver},
};

#[derive(Debug)]
pub enum Fetch {
    Object(Object),
    /// The publisher finished the stream (FIN): every object of the range
    /// was delivered.
    End,
}

#[derive(Debug)]
pub struct FetchDataReceiver {
    stream_receiver: UniStreamReceiver,
    pub request_id: u64,
}

impl FetchDataReceiver {
    pub(crate) fn new(stream: UniStreamReceiver, fetch_header: FetchHeader) -> Self {
        Self {
            stream_receiver: stream,
            request_id: fetch_header.request_id,
        }
    }

    pub async fn receive(&mut self) -> anyhow::Result<Fetch> {
        match self.stream_receiver.receive().await {
            Ok(Some(UniStreamData::Fetch(FetchFrame::Object(field)))) => {
                Ok(Fetch::Object(field.into_object()))
            }
            // The decoder yields the header once, and the stream task took it.
            Ok(Some(UniStreamData::Fetch(FetchFrame::Header(_)))) => {
                unreachable!("a FETCH stream has one header")
            }
            Ok(Some(UniStreamData::Subgroup(_))) => {
                unreachable!("Unexpected subgroup data in fetch stream")
            }
            Ok(None) => Ok(Fetch::End),
            Err(error) => {
                match &error {
                    StreamReceiveError::Decode(_) | StreamReceiveError::ProtocolViolation(_) => {
                        tracing::error!(%error, "Failed to decode the fetch stream")
                    }
                    StreamReceiveError::Closed(_) => tracing::debug!(%error, "Fetch stream ended"),
                }
                anyhow::bail!("Failed to receive data from fetch stream: {error}")
            }
        }
    }
}
