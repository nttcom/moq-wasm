use moqt::object::{DataStreamResetCode, StreamPriority, SubgroupPart};

use crate::modules::session::data_sender::DataSender;

enum SenderInner {
    Uninitialized(moqt::object::SubgroupHeaderSender),
    HeaderSent(moqt::object::SubgroupObjectSender),
}

pub(crate) struct StreamSender {
    inner: Option<SenderInner>,
}

impl StreamSender {
    pub(crate) fn new(inner: moqt::object::SubgroupHeaderSender) -> Self {
        Self {
            inner: Some(SenderInner::Uninitialized(inner)),
        }
    }

    fn header_sent(&mut self) -> anyhow::Result<&mut moqt::object::SubgroupObjectSender> {
        match self.inner.as_mut() {
            Some(SenderInner::HeaderSent(sender)) => Ok(sender),
            _ => Err(anyhow::anyhow!("Header not set for StreamSender")),
        }
    }
}

#[async_trait::async_trait]
impl DataSender for StreamSender {
    async fn send_object(&mut self, object: SubgroupPart) -> anyhow::Result<()> {
        match object {
            SubgroupPart::Object(object) => self.header_sent()?.send_object(&object).await,
            SubgroupPart::ObjectStart {
                object,
                payload_len,
            } => self.header_sent()?.begin_object(&object, payload_len).await,
            SubgroupPart::Payload { data, .. } => self.header_sent()?.send_payload(&data).await,
            SubgroupPart::Header(start) => match self.inner.take() {
                Some(SenderInner::Uninitialized(sender)) => {
                    let sent = sender.start(start).await?;
                    self.inner = Some(SenderInner::HeaderSent(sent));
                    Ok(())
                }
                _ => Err(anyhow::anyhow!(
                    "StreamSender already initialized or in invalid state"
                )),
            },
        }
    }

    async fn set_priority(&mut self, priority: StreamPriority) -> anyhow::Result<()> {
        match self.inner.as_ref() {
            Some(SenderInner::Uninitialized(sender)) => sender.set_priority(priority).await,
            Some(SenderInner::HeaderSent(sender)) => sender.set_priority(priority).await,
            None => Ok(()),
        }
    }

    async fn close(&mut self) -> anyhow::Result<()> {
        match self.inner.as_mut() {
            Some(SenderInner::Uninitialized(sender)) => sender.close().await,
            Some(SenderInner::HeaderSent(sender)) => sender.close().await,
            None => Ok(()),
        }
    }

    async fn reset(&mut self, error_code: DataStreamResetCode) -> anyhow::Result<()> {
        match self.inner.as_mut() {
            Some(SenderInner::Uninitialized(sender)) => sender.reset(error_code).await,
            Some(SenderInner::HeaderSent(sender)) => sender.reset(error_code).await,
            None => Ok(()),
        }
    }
}
