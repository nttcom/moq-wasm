use crate::modules::session::{data_object::DataObject, data_sender::DataSender};

#[async_trait::async_trait]
impl DataSender for moqt::DatagramSender {
    async fn send_object(&mut self, object: DataObject) -> anyhow::Result<()> {
        match object {
            DataObject::ObjectDatagram(datagram) => self.send(datagram).await,
            _ => Err(anyhow::anyhow!("Invalid object type for DatagramSender")),
        }
    }
}
