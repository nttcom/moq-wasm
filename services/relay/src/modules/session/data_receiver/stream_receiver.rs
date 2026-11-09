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
impl StreamReceiver for moqt::object::SubgroupPartReceiver {
    async fn receive_object(
        &mut self,
    ) -> Result<Option<DataObject>, moqt::object::StreamReceiveError> {
        let part = self.receive().await?;
        Ok(part.map(|part| match part {
            moqt::object::SubgroupPart::Header(start) => {
                tracing::debug!(subgroup_header = ?start, "Received subgroup header");
                DataObject::SubgroupHeader(start)
            }
            moqt::object::SubgroupPart::Object(object) => DataObject::SubgroupObject(object),
            moqt::object::SubgroupPart::ObjectStart {
                object,
                payload_len,
            } => DataObject::SubgroupObjectStart {
                object,
                payload_len,
            },
            moqt::object::SubgroupPart::Payload { data, last } => {
                DataObject::SubgroupPayload { data, last }
            }
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
            moqt::object::StreamDataReceiverFactory::next(self)
                .await?
                .into_parts(),
        ))
    }
}
