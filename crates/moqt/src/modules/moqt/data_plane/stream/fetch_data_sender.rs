use crate::modules::moqt::control_plane::error_codes::DataStreamResetCode;
use crate::modules::moqt::{
    data_plane::{object::canonical_object::Object, stream::stream_sender::StreamSender},
    version::Version,
};
use crate::modules::transport::transport_connection::BoxedSendStream;

pub struct FetchDataSender {
    stream_sender: StreamSender,
    version: Version,
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
            version,
        })
    }

    /// Sends `object` on this FETCH response. A datagram object is sent with
    /// its Object ID as the Subgroup ID (draft-14 §10.4.4), and a zero-length
    /// object as its status.
    pub async fn send_object(&self, object: &Object) -> anyhow::Result<()> {
        self.stream_sender
            .send(&self.version.encode_fetch_object(object))
            .await
    }

    pub async fn close(&self) -> anyhow::Result<()> {
        self.stream_sender.close().await
    }

    pub async fn reset(&self, code: DataStreamResetCode) -> anyhow::Result<()> {
        self.stream_sender.reset(code.into()).await
    }
}
