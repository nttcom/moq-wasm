use crate::modules::session::{data_object::DataObject, data_sender::DataSender};

enum SenderInner {
    Uninitialized(moqt::SubgroupHeaderSender),
    HeaderSent(moqt::SubgroupObjectSender),
}

pub(crate) struct StreamSender {
    inner: Option<SenderInner>,
    subscriber_track_alias: u64,
}

impl StreamSender {
    pub(crate) fn new(inner: moqt::SubgroupHeaderSender, subscriber_track_alias: u64) -> Self {
        Self {
            inner: Some(SenderInner::Uninitialized(inner)),
            subscriber_track_alias,
        }
    }
}

#[async_trait::async_trait]
impl DataSender for StreamSender {
    async fn send_object(&mut self, object: DataObject) -> anyhow::Result<()> {
        match object {
            DataObject::SubgroupObject(field) => match self.inner.as_mut() {
                Some(SenderInner::HeaderSent(sender)) => sender.send(field).await,
                _ => Err(anyhow::anyhow!("Header not set for StreamSender")),
            },
            DataObject::SubgroupHeader(mut header) => {
                header.track_alias = self.subscriber_track_alias;
                match self.inner.take() {
                    Some(SenderInner::Uninitialized(sender)) => {
                        let sent = sender.send_header(header).await?;
                        self.inner = Some(SenderInner::HeaderSent(sent));
                        Ok(())
                    }
                    _ => Err(anyhow::anyhow!(
                        "StreamSender already initialized or in invalid state"
                    )),
                }
            }
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

    async fn reset(&mut self, error_code: u64) -> anyhow::Result<()> {
        match self.inner.as_mut() {
            Some(SenderInner::Uninitialized(sender)) => sender.reset(error_code).await,
            Some(SenderInner::HeaderSent(sender)) => sender.reset(error_code).await,
            None => Ok(()),
        }
    }
}
