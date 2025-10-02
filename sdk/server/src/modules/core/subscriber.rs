use async_trait::async_trait;

#[async_trait]
pub(crate) trait Subscriber: 'static + Send + Sync {
    async fn subscribe_namespace(&self, namespaces: Vec<String>) -> anyhow::Result<()>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> Subscriber for moqt::Subscriber<T> {
    async fn subscribe_namespace(&self, namespaces: Vec<String>) -> anyhow::Result<()> {
        self.subscribe_namespace(namespaces).await
    }
}
