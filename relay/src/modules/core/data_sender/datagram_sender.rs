use crate::modules::core::{data_object::DataObject, data_sender::DataSender};

pub(crate) struct DatagramSender<T: moqt::TransportProtocol> {
    inner: moqt::DatagramSender<T>,
}

impl<T: moqt::TransportProtocol> DatagramSender<T> {
    pub(crate) fn new(inner: moqt::DatagramSender<T>) -> Self {
        Self { inner }
    }

    pub(crate) async fn send(&mut self, object: DataObject) -> anyhow::Result<()> {
        match object {
            DataObject::ObjectDatagram(datagram) => self.inner.send(datagram).await,
            _ => Err(anyhow::anyhow!("Invalid object type for DatagramSender")),
        }
    }
}

#[async_trait::async_trait]
impl<T: moqt::TransportProtocol> DataSender for DatagramSender<T> {
    async fn send_object(&mut self, object: DataObject) -> anyhow::Result<()> {
        self.send(object).await
    }
}
