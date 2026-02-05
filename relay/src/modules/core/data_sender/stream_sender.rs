use crate::modules::core::{data_object::DataObject, data_sender::DataSender};

pub(crate) struct StreamSender<T: moqt::TransportProtocol> {
    inner: moqt::StreamDataSender<T>,
    header: Option<moqt::SubgroupHeader>,
}

impl<T: moqt::TransportProtocol> StreamSender<T> {
    pub(crate) fn new(inner: moqt::StreamDataSender<T>) -> Self {
        Self {
            inner,
            header: None,
        }
    }

    fn set_header(&mut self, object: moqt::SubgroupHeader) -> anyhow::Result<()> {
        self.header = Some(object);
        Ok(())
    }

    pub(crate) async fn send(&mut self, object: DataObject) -> anyhow::Result<()> {
        match object {
            DataObject::SubgroupObject(field) => {
                if let Some(header) = &self.header {
                    self.inner.send(header.clone(), field).await
                } else {
                    Err(anyhow::anyhow!("Header not set for StreamSender"))
                }
            }
            DataObject::SubgroupHeader(header) => self.set_header(header),
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
