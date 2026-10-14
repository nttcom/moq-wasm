use crate::modules::session::{data_object::DataObject, data_sender::DataSender};
use moqt::object::DataStreamResetCode;

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
}

#[async_trait::async_trait]
impl DataSender for StreamSender {
    async fn send_object(&mut self, object: DataObject) -> anyhow::Result<()> {
        match object {
            DataObject::SubgroupObject(object) => match self.inner.as_mut() {
                Some(SenderInner::HeaderSent(sender)) => sender.send_object(&object).await,
                _ => Err(anyhow::anyhow!("Header not set for StreamSender")),
            },
            DataObject::SubgroupHeader(start) => match self.inner.take() {
                Some(SenderInner::Uninitialized(sender)) => {
                    let sent = sender.start(start).await?;
                    self.inner = Some(SenderInner::HeaderSent(sent));
                    Ok(())
                }
                _ => Err(anyhow::anyhow!(
                    "StreamSender already initialized or in invalid state"
                )),
            },
            _ => Err(anyhow::anyhow!("Invalid object type for StreamSender")),
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
