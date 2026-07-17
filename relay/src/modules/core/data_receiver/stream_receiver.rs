use crate::modules::core::data_object::DataObject;

#[async_trait::async_trait]
pub(crate) trait StreamReceiver: Send + Sync + 'static {
    /// `Ok(None)` means the stream finished normally (FIN); errors keep the
    /// transport-closed / decode-failed distinction from `StreamReceiveError`.
    async fn receive_object(&mut self) -> Result<Option<DataObject>, moqt::StreamReceiveError>;
}

#[async_trait::async_trait]
impl<T: moqt::TransportProtocol> StreamReceiver for moqt::StreamDataReceiver<T> {
    async fn receive_object(&mut self) -> Result<Option<DataObject>, moqt::StreamReceiveError> {
        let object = self.receive().await?;
        Ok(object.map(|object| match object {
            moqt::Subgroup::Header(header) => {
                tracing::debug!(subgroup_header = ?header, "Received subgroup header");
                DataObject::SubgroupHeader(header)
            }
            moqt::Subgroup::Object(field) => DataObject::SubgroupObject(field),
        }))
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
