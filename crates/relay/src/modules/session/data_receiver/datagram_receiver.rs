use crate::modules::session::data_object::DataObject;

#[async_trait::async_trait]
pub(crate) trait DatagramReceiver: Send + Sync + 'static {
    async fn receive_object(&mut self) -> anyhow::Result<DataObject>;
}

#[async_trait::async_trait]
impl DatagramReceiver for moqt::DatagramReceiver {
    async fn receive_object(&mut self) -> anyhow::Result<DataObject> {
        let object = self.receive().await?;
        Ok(DataObject::ObjectDatagram(object))
    }
}
