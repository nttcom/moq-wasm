use crate::{
    TransportProtocol,
    modules::moqt::data_plane::{
        object::fetch::{FetchHeader, FetchObjectField},
        stream::stream_sender::StreamSender,
    },
};

pub struct FetchDataSender<T: TransportProtocol> {
    stream_sender: StreamSender<T>,
}

impl<T: TransportProtocol> FetchDataSender<T> {
    pub async fn new(send_stream: T::SendStream, header: FetchHeader) -> anyhow::Result<Self> {
        let stream_sender = StreamSender::new(send_stream);
        let header_bytes = header.encode();
        stream_sender.send(&header_bytes).await?;
        Ok(Self { stream_sender })
    }

    pub async fn send(&self, object: FetchObjectField) -> anyhow::Result<()> {
        let bytes = object.encode();
        self.stream_sender.send(&bytes).await
    }

    pub async fn close(&self) -> anyhow::Result<()> {
        self.stream_sender.close().await
    }

    pub async fn reset(&self, error_code: u64) -> anyhow::Result<()> {
        self.stream_sender.reset(error_code).await
    }
}
