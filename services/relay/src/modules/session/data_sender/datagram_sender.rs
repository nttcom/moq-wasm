use moqt::object::{DatagramDelivery, Object};

#[async_trait::async_trait]
pub(crate) trait DatagramSender: 'static + Send + Sync {
    async fn send(&mut self, object: Object) -> anyhow::Result<DatagramDelivery>;
}

#[async_trait::async_trait]
impl DatagramSender for moqt::object::DatagramSender {
    async fn send(&mut self, object: Object) -> anyhow::Result<DatagramDelivery> {
        self.send_object(&object).await
    }
}
