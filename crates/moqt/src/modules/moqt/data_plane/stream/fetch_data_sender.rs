use crate::modules::moqt::control_plane::error_codes::DataStreamResetCode;
use crate::modules::moqt::data_plane::{
    object::{
        canonical_object::Object,
        fetch::{FetchHeader, FetchObjectField},
    },
    stream::stream_sender::StreamSender,
};
use crate::modules::transport::transport_connection::BoxedSendStream;

pub struct FetchDataSender {
    stream_sender: StreamSender,
}

impl FetchDataSender {
    pub(crate) async fn new(
        send_stream: BoxedSendStream,
        header: FetchHeader,
    ) -> anyhow::Result<Self> {
        let stream_sender = StreamSender::new(send_stream);
        let header_bytes = header.encode();
        stream_sender.send(&header_bytes).await?;
        Ok(Self { stream_sender })
    }

    /// Sends `object` on this FETCH response. A datagram object is sent with
    /// its Object ID as the Subgroup ID (draft-14 §10.4.4), and a zero-length
    /// object as its status.
    pub async fn send_object(&self, object: &Object) -> anyhow::Result<()> {
        let bytes = FetchObjectField::from_object(object).encode();
        self.stream_sender.send(&bytes).await
    }

    pub async fn close(&self) -> anyhow::Result<()> {
        self.stream_sender.close().await
    }

    pub async fn reset(&self, code: DataStreamResetCode) -> anyhow::Result<()> {
        self.stream_sender.reset(code.into()).await
    }
}
