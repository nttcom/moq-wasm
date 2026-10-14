use crate::modules::moqt::control_plane::error_codes::DataStreamResetCode;
use crate::modules::moqt::{
    data_plane::{object::canonical_object::Object, stream::stream_sender::StreamSender},
    version::{FetchEncoder, Version},
};
use crate::modules::transport::transport_connection::BoxedSendStream;

pub struct FetchDataSender {
    stream_sender: StreamSender,
    encoder: std::sync::Mutex<FetchEncoder>,
}

impl FetchDataSender {
    pub(crate) async fn new(
        version: Version,
        send_stream: BoxedSendStream,
        request_id: u64,
    ) -> anyhow::Result<Self> {
        let stream_sender = StreamSender::new(send_stream);
        stream_sender
            .send(&version.encode_fetch_header(request_id))
            .await?;
        Ok(Self {
            stream_sender,
            encoder: std::sync::Mutex::new(FetchEncoder::new(version)),
        })
    }

    /// Sends `object` on this FETCH response. draft-14 sends a datagram
    /// object with its Object ID as the Subgroup ID (§10.4.4) and a
    /// zero-length object as its status; draft-18 carries only Normal
    /// objects and skips the others.
    pub async fn send_object(&self, object: &Object) -> anyhow::Result<()> {
        let encoded = self
            .encoder
            .lock()
            .expect("fetch encoder poisoned")
            .object(object)?;
        match encoded {
            Some(bytes) => self.stream_sender.send(&bytes).await,
            None => Ok(()),
        }
    }

    pub async fn close(&self) -> anyhow::Result<()> {
        self.stream_sender.close().await
    }

    pub async fn reset(&self, code: DataStreamResetCode) -> anyhow::Result<()> {
        let version = self
            .encoder
            .lock()
            .expect("fetch encoder poisoned")
            .version();
        self.stream_sender
            .reset(version.data_stream_reset_code(code))
            .await
    }
}
