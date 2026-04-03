use crate::modules::core::{data_sender::DataSender, data_sender::stream_sender::StreamSender};

#[async_trait::async_trait]
pub(crate) trait StreamSenderFactory: Send + 'static {
    async fn next(&mut self) -> anyhow::Result<Box<dyn DataSender>>;
}

pub(crate) struct ConcreteStreamSenderFactory<T: moqt::TransportProtocol> {
    inner: moqt::StreamDataSenderFactory<T>,
    subscriber_track_alias: u64,
}

impl<T: moqt::TransportProtocol> ConcreteStreamSenderFactory<T> {
    pub(crate) fn new(
        inner: moqt::StreamDataSenderFactory<T>,
        subscriber_track_alias: u64,
    ) -> Self {
        Self {
            inner,
            subscriber_track_alias,
        }
    }
}

#[async_trait::async_trait]
impl<T: moqt::TransportProtocol> StreamSenderFactory for ConcreteStreamSenderFactory<T> {
    async fn next(&mut self) -> anyhow::Result<Box<dyn DataSender>> {
        let sender = self.inner.next().await?;
        Ok(Box::new(StreamSender::new(
            sender,
            self.subscriber_track_alias,
        )))
    }
}
