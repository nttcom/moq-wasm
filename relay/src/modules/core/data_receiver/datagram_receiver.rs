use crate::modules::core::{data_object::DataObject, data_receiver::DataReceiver};

#[derive(Debug)]
pub(crate) struct DatagramReceiver<T: moqt::TransportProtocol> {
    inner: moqt::DatagramReceiver<T>,
}

impl<T: moqt::TransportProtocol> DatagramReceiver<T> {
    pub(crate) fn new(inner: moqt::DatagramReceiver<T>) -> Self {
        Self { inner }
    }

    fn track_alias(&self) -> u64 {
        self.inner.track_alias
    }

    async fn receive(&mut self) -> anyhow::Result<DataObject> {
        let object = self.inner.receive().await?;
        Ok(DataObject::ObjectDatagram(object))
    }
}

#[async_trait::async_trait]
impl<T: moqt::TransportProtocol> DataReceiver for DatagramReceiver<T> {
    fn get_track_alias(&self) -> u64 {
        self.track_alias()
    }

    fn datagram(&self) -> bool {
        true
    }

    async fn receive_object(&mut self) -> anyhow::Result<DataObject> {
        self.receive().await
    }
}
