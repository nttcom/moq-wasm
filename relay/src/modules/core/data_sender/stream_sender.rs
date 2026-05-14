use crate::modules::core::{data_object::DataObject, data_sender::DataSender};

/// Internal enum that holds the `StreamDataSender` across its typestate transitions.
/// Since `send_header` consumes `self`, we wrap it in `Option` to allow `take()`.
enum SenderInner<T: moqt::TransportProtocol> {
    Uninitialized(moqt::SubgroupHeaderSender<T>),
    HeaderSent(moqt::SubgroupObjectSender<T>),
}

pub(crate) struct StreamSender<T: moqt::TransportProtocol> {
    inner: Option<SenderInner<T>>,
    subscriber_track_alias: u64,
}

impl<T: moqt::TransportProtocol> StreamSender<T> {
    pub(crate) fn new(inner: moqt::SubgroupHeaderSender<T>, subscriber_track_alias: u64) -> Self {
        Self {
            inner: Some(SenderInner::Uninitialized(inner)),
            subscriber_track_alias,
        }
    }

    pub(crate) async fn send(&mut self, object: DataObject) -> anyhow::Result<()> {
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
}

#[async_trait::async_trait]
impl<T: moqt::TransportProtocol> DataSender for StreamSender<T> {
    async fn send_object(&mut self, object: DataObject) -> anyhow::Result<()> {
        self.send(object).await
    }
}
