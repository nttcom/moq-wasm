use crate::modules::session::data_object::DataObject;

#[async_trait::async_trait]
pub(crate) trait StreamReceiver: Send + Sync + 'static {
    /// `Ok(None)` means the stream finished normally (FIN); errors keep the
    /// transport-closed / decode-failed distinction from `StreamReceiveError`.
    async fn receive_object(
        &mut self,
    ) -> Result<Option<DataObject>, moqt::object::StreamReceiveError>;
}

#[async_trait::async_trait]
impl StreamReceiver for moqt::object::StreamDataReceiver {
    async fn receive_object(
        &mut self,
    ) -> Result<Option<DataObject>, moqt::object::StreamReceiveError> {
        let object = self.receive().await?;
        Ok(object.map(|object| match object {
            moqt::object::Subgroup::Header(header) => {
                tracing::debug!(subgroup_header = ?header, "Received subgroup header");
                DataObject::SubgroupHeader(header)
            }
            moqt::object::Subgroup::Object(field) => DataObject::SubgroupObject(field),
        }))
    }
}

#[async_trait::async_trait]
pub(crate) trait StreamReceiverFactory: Send + 'static {
    async fn next(&mut self) -> anyhow::Result<Box<dyn StreamReceiver>>;
}

#[async_trait::async_trait]
impl StreamReceiverFactory for moqt::object::StreamDataReceiverFactory {
    async fn next(&mut self) -> anyhow::Result<Box<dyn StreamReceiver>> {
        Ok(Box::new(
            moqt::object::StreamDataReceiverFactory::next(self).await?,
        ))
    }
}
