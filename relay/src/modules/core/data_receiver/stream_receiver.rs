use crate::modules::core::data_object::DataObject;

#[async_trait::async_trait]
pub(crate) trait StreamReceiver: Send + Sync + 'static {
    async fn receive_object(&mut self) -> anyhow::Result<DataObject>;
}

#[async_trait::async_trait]
impl<T: moqt::TransportProtocol> StreamReceiver for moqt::StreamDataReceiver<T> {
    async fn receive_object(&mut self) -> anyhow::Result<DataObject> {
        let object = self.receive().await?;
        match object {
            moqt::Subgroup::Header(header) => {
                tracing::debug!(subgroup_header = ?header, "Received subgroup header");
                Ok(DataObject::SubgroupHeader(header))
            }
            moqt::Subgroup::Object(field) => Ok(DataObject::SubgroupObject(field)),
        }
    }
}

#[async_trait::async_trait]
pub(crate) trait StreamReceiverFactory: Send + 'static {
    async fn next(&mut self) -> anyhow::Result<Box<dyn StreamReceiver>>;
}

#[async_trait::async_trait]
impl<T: moqt::TransportProtocol> StreamReceiverFactory for moqt::StreamDataReceiverFactory<T> {
    async fn next(&mut self) -> anyhow::Result<Box<dyn StreamReceiver>> {
        Ok(Box::new(moqt::StreamDataReceiverFactory::next(self).await?))
    }
}
