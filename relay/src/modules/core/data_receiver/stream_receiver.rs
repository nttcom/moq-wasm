use crate::modules::core::data_object::DataObject;

#[async_trait::async_trait]
pub(crate) trait StreamReceiver: Send + Sync + 'static {
    fn get_track_alias(&self) -> u64;
    async fn receive_object(&mut self) -> anyhow::Result<DataObject>;
}

#[async_trait::async_trait]
impl<T: moqt::TransportProtocol> StreamReceiver for moqt::StreamDataReceiver<T> {
    fn get_track_alias(&self) -> u64 {
        self.track_alias
    }

    async fn receive_object(&mut self) -> anyhow::Result<DataObject> {
        let object = self.receive().await?;
        match object {
            moqt::Subgroup::Header(header) => Ok(DataObject::SubgroupHeader(header)),
            moqt::Subgroup::Object(field) => Ok(DataObject::SubgroupObject(field)),
        }
    }
}
