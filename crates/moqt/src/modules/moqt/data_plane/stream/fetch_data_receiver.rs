use crate::modules::moqt::{
    data_plane::{object::canonical_object::Object, stream::stream_receiver::StreamReceiveError},
    version::FetchReader,
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
    pub request_id: u64,
    reader: FetchReader,
}

impl FetchDataReceiver {
    pub(crate) fn new(request_id: u64, reader: FetchReader) -> Self {
        Self { request_id, reader }
    }

    pub async fn receive(&mut self) -> anyhow::Result<Fetch> {
        match self.reader.next_object().await {
            Ok(Some(object)) => Ok(Fetch::Object(object)),
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
