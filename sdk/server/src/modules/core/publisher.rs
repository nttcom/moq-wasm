use async_trait::async_trait;

#[async_trait]
pub(crate) trait Publisher: 'static + Send + Sync {
    async fn send_publish_namespace(&self, namespaces: Vec<String>) -> anyhow::Result<()>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> Publisher for moqt::Publisher<T> {
    async fn send_publish_namespace(&self, namespaces: Vec<String>) -> anyhow::Result<()> {
        self.publish_namespace(namespaces).await
    }
}
