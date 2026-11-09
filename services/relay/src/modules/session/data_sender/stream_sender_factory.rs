use crate::modules::session::{data_sender::DataSender, data_sender::stream_sender::StreamSender};

#[async_trait::async_trait]
pub(crate) trait StreamSenderFactory: Send + 'static {
    async fn next(
        &mut self,
        priority: moqt::object::StreamPriority,
    ) -> anyhow::Result<Box<dyn DataSender>>;
}

#[async_trait::async_trait]
impl StreamSenderFactory for moqt::object::StreamDataSenderFactory {
    async fn next(
        &mut self,
        priority: moqt::object::StreamPriority,
    ) -> anyhow::Result<Box<dyn DataSender>> {
        let sender = moqt::object::StreamDataSenderFactory::next(self).await?;
        sender.set_priority(priority).await?;
        Ok(Box::new(StreamSender::new(sender)))
    }
}
