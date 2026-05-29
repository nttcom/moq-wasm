use async_trait::async_trait;

use crate::modules::core::{
    data_sender::{
        DataSender,
        datagram_sender::DatagramSender,
        stream_sender_factory::{ConcreteStreamSenderFactory, StreamSenderFactory},
    },
    subscription::DownstreamSubscription,
};

#[async_trait]
pub(crate) trait Publisher: 'static + Send + Sync {
    async fn send_publish_namespace(&self, namespaces: String) -> anyhow::Result<()>;
    async fn send_publish(
        &self,
        track_namespace: String,
        track_name: String,
    ) -> anyhow::Result<DownstreamSubscription>;
    fn new_stream_factory(
        &self,
        downstream_subscription: &DownstreamSubscription,
    ) -> Box<dyn StreamSenderFactory>;
    fn new_datagram(&self, downstream_subscription: &DownstreamSubscription) -> Box<dyn DataSender>;
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
    ) -> anyhow::Result<DownstreamSubscription> {
        let option = moqt::PublishOption::default();
        let subscription = self.publish(track_namespace, track_name, option).await?;
        Ok(DownstreamSubscription::from(subscription))
    }

    fn new_stream_factory(
        &self,
        downstream_subscription: &DownstreamSubscription,
    ) -> Box<dyn StreamSenderFactory> {
        let subscriber_track_alias = downstream_subscription.track_alias();
        let inner = self.create_stream(downstream_subscription.as_moqt());
        Box::new(ConcreteStreamSenderFactory::new(
            inner,
            subscriber_track_alias,
        ))
    }

    fn new_datagram(&self, downstream_subscription: &DownstreamSubscription) -> Box<dyn DataSender> {
        let sender = self.create_datagram(downstream_subscription.as_moqt());
        let sender = DatagramSender::new(sender);
        Box::new(sender)
    }
}
