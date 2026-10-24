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
            moqt::object::Subgroup::Header(start) => {
                tracing::debug!(subgroup_header = ?start, "Received subgroup header");
                DataObject::SubgroupHeader(start)
            }
            moqt::object::Subgroup::Object(object) => DataObject::SubgroupObject(object),
        }))
    }
}
