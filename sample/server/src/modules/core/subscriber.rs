use async_trait::async_trait;

use crate::modules::core::subscription::Subscription;

#[async_trait]
pub(crate) trait Subscriber: 'static + Send + Sync {
    async fn send_subscribe_namespace(&self, namespaces: String) -> anyhow::Result<()>;
    async fn send_subscribe(
        &self,
        track_namespace: String,
        track_name: String,
    ) -> anyhow::Result<Box<dyn Subscription>>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> Subscriber for moqt::Subscriber<T> {
    async fn send_subscribe_namespace(&self, namespace: String) -> anyhow::Result<()> {
        self.subscribe_namespace(namespace).await
    }
    async fn send_subscribe(
        &self,
        track_namespace: String,
        track_name: String,
    ) -> anyhow::Result<Box<dyn Subscription>> {
        let result = self
            .subscribe(
                track_namespace,
                track_name,
                moqt::SubscribeOption::default(),
            )
            .await?;
        Ok(Box::new(result))
    }
}
