use async_trait::async_trait;

use crate::modules::core::publication::Publication;

#[async_trait]
pub(crate) trait Publisher: 'static + Send + Sync {
    async fn send_publish_namespace(&self, namespaces: String) -> anyhow::Result<()>;
    async fn send_publish(
        &self,
        track_namespace: String,
        track_name: String,
        track_alias: u64,
    ) -> anyhow::Result<Box<dyn Publication>>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> Publisher for moqt::Publisher<T> {
    async fn send_publish_namespace(&self, namespaces: String) -> anyhow::Result<()> {
        self.publish_namespace(namespaces).await
    }

    async fn send_publish(
        &self,
        track_namespace: String,
        track_name: String,
        track_alias: u64,
    ) -> anyhow::Result<Box<dyn Publication>> {
        let mut option = moqt::PublishOption::default();
        option.track_alias = track_alias;
        let result = self.publish(track_namespace, track_name, option).await?;
        Ok(Box::new(result))
    }
}
