use async_trait::async_trait;

use crate::modules::core::{
    data_sender::{DataSender, datagram_sender::DatagramSender, stream_sender::StreamSender},
    published_resource::PublishedResource,
};

#[async_trait]
pub(crate) trait Publisher: 'static + Send + Sync {
    async fn send_publish_namespace(&self, namespaces: String) -> anyhow::Result<()>;
    async fn send_publish(
        &self,
        track_namespace: String,
        track_name: String,
    ) -> anyhow::Result<PublishedResource>;
    async fn new_stream(
        &self,
        published_resource: &PublishedResource,
        subscriber_track_alias: u64,
    ) -> anyhow::Result<Box<dyn DataSender>>;
    fn new_datagram(&self, published_resource: &PublishedResource) -> Box<dyn DataSender>;
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
    ) -> anyhow::Result<PublishedResource> {
        let option = moqt::PublishOption::default();
        let result = self.publish(track_namespace, track_name, option).await?;
        Ok(PublishedResource::from(result))
    }

    async fn new_stream(
        &self,
        published_resource: &PublishedResource,
        subscriber_track_alias: u64,
    ) -> anyhow::Result<Box<dyn DataSender>> {
        let sender = self.create_stream(published_resource.as_moqt()).await?;
        let sender = StreamSender::new(sender, subscriber_track_alias);
        Ok(Box::new(sender))
    }

    fn new_datagram(&self, published_resource: &PublishedResource) -> Box<dyn DataSender> {
        let sender = self.create_datagram(published_resource.as_moqt());
        let sender = DatagramSender::new(sender);
        Box::new(sender)
    }
}
