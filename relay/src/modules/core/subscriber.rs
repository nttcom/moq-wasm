use async_trait::async_trait;

use crate::modules::core::{
    data_receiver::{
        DataReceiver, datagram_receiver::DatagramReceiver, stream_receiver::StreamReceiver,
    },
    handler::publish::SubscribeOption,
    subscription::Subscription,
};

#[async_trait]
pub(crate) trait Subscriber: 'static + Send + Sync {
    async fn _send_subscribe_namespace(&self, namespaces: String) -> anyhow::Result<()>;
    async fn send_subscribe(
        &self,
        track_namespace: String,
        track_name: String,
        option: SubscribeOption,
    ) -> anyhow::Result<Subscription>;
    async fn create_data_receiver(
        &self,
        subscription: Subscription,
    ) -> anyhow::Result<Box<dyn DataReceiver>>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> Subscriber for moqt::Subscriber<T> {
    #[tracing::instrument(
        level = "info",
        name = "relay.subscriber.send_subscribe_namespace",
        skip_all,
        fields(namespace = %namespace)
    )]
    async fn _send_subscribe_namespace(&self, namespace: String) -> anyhow::Result<()> {
        self.subscribe_namespace(namespace).await
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.subscriber.send_subscribe",
        skip_all,
        fields(track_namespace = %track_namespace, track_name = %track_name, subscriber_priority = option.subscriber_priority, forward = option.forward)
    )]
    async fn send_subscribe(
        &self,
        track_namespace: String,
        track_name: String,
        option: SubscribeOption,
    ) -> anyhow::Result<Subscription> {
        let option = moqt::SubscribeOption {
            subscriber_priority: option.subscriber_priority,
            group_order: option.group_order.as_moqt(),
            forward: option.forward,
            filter_type: option.filter_type.as_moqt(),
        };
        let subscription = self.subscribe(track_namespace, track_name, option).await?;
        Ok(Subscription::from(subscription))
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.subscriber.create_data_receiver",
        skip_all,
        fields(track_alias = subscription.track_alias())
    )]
    async fn create_data_receiver(
        &self,
        subscription: Subscription,
    ) -> anyhow::Result<Box<dyn DataReceiver>> {
        let result = self.accept_data_receiver(subscription.as_moqt()).await?;
        match result {
            moqt::DataReceiver::Stream(inner) => {
                let receiver = StreamReceiver::new(inner);
                Ok(Box::new(receiver))
            }
            moqt::DataReceiver::Datagram(inner) => {
                let receiver = DatagramReceiver::new(inner);
                Ok(Box::new(receiver))
            }
        }
    }
}
