use crate::modules::session::{data_sender::DataSender, data_sender::stream_sender::StreamSender};

#[async_trait::async_trait]
pub(crate) trait StreamSenderFactory: Send + 'static {
    async fn next(&mut self, priority: moqt::StreamPriority)
    -> anyhow::Result<Box<dyn DataSender>>;
}

pub(crate) struct ConcreteStreamSenderFactory {
    inner: moqt::StreamDataSenderFactory,
    subscriber_track_alias: u64,
}

impl ConcreteStreamSenderFactory {
    pub(crate) fn new(inner: moqt::StreamDataSenderFactory, subscriber_track_alias: u64) -> Self {
        Self {
            inner,
            subscriber_track_alias,
        }
    }
}

#[async_trait::async_trait]
impl StreamSenderFactory for ConcreteStreamSenderFactory {
    async fn next(
        &mut self,
        priority: moqt::StreamPriority,
    ) -> anyhow::Result<Box<dyn DataSender>> {
        let sender = self.inner.next().await?;
        sender.set_priority(priority).await?;
        Ok(Box::new(StreamSender::new(
            sender,
            self.subscriber_track_alias,
        )))
    }
}
