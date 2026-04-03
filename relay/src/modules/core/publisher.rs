use async_trait::async_trait;

use crate::modules::core::{
    data_sender::{
        DataSender,
        datagram_sender::DatagramSender,
        stream_sender_factory::{ConcreteStreamSenderFactory, StreamSenderFactory},
    },
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
    fn new_stream_factory(
        &self,
        published_resource: &PublishedResource,
    ) -> Box<dyn StreamSenderFactory>;
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

    fn new_stream_factory(
        &self,
        published_resource: &PublishedResource,
    ) -> Box<dyn StreamSenderFactory> {
        let subscriber_track_alias = published_resource.track_alias();
        let inner = self.create_stream(published_resource.as_moqt());
        Box::new(ConcreteStreamSenderFactory::new(
            inner,
            subscriber_track_alias,
        ))
    }

    fn new_datagram(&self, published_resource: &PublishedResource) -> Box<dyn DataSender> {
        let sender = self.create_datagram(published_resource.as_moqt());
        let sender = DatagramSender::new(sender);
        Box::new(sender)
    }
}
