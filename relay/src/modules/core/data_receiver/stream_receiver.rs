use crate::modules::core::{data_object::DataObject, data_receiver::DataReceiver};

#[derive(Debug)]
pub(crate) struct StreamReceiver<T: moqt::TransportProtocol> {
    inner: moqt::StreamDataReceiver<T>,
}

impl<T: moqt::TransportProtocol> StreamReceiver<T> {
    pub(crate) fn new(inner: moqt::StreamDataReceiver<T>) -> Self {
        Self { inner }
    }

    fn track_alias(&self) -> u64 {
        self.inner.track_alias
    }

    async fn receive(&mut self) -> anyhow::Result<DataObject> {
        let object = self.inner.receive().await?;
        match object {
            moqt::Subgroup::Header(header) => Ok(DataObject::SubgroupHeader(header)),
            moqt::Subgroup::Object(field) => Ok(DataObject::SubgroupObject(field)),
        }
    }
}

#[async_trait::async_trait]
impl<T: moqt::TransportProtocol> DataReceiver for StreamReceiver<T> {
    fn get_track_alias(&self) -> u64 {
        self.track_alias()
    }

    fn datagram(&self) -> bool {
        false
    }

    async fn receive_object(&mut self) -> anyhow::Result<DataObject> {
        self.receive().await
    }
}
