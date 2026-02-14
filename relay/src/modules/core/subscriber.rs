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
    async fn send_subscribe_namespace(&self, namespaces: String) -> anyhow::Result<()>;
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
    async fn send_subscribe_namespace(&self, namespace: String) -> anyhow::Result<()> {
        self.subscribe_namespace(namespace).await
    }

    async fn send_subscribe(
        &self,
        track_namespace: String,
        track_name: String,
        option: SubscribeOption,
    ) -> anyhow::Result<Subscription> {
        let option = moqt::SubscribeOption {
            subscriber_priority: option.subscriber_priority,
            group_order: option.group_order.into_moqt(),
            forward: option.forward,
            filter_type: option.filter_type.into_moqt(),
        };
        let subscription = self.subscribe(track_namespace, track_name, option).await?;
        Ok(Subscription::from(subscription))
    }

    async fn create_data_receiver(
        &self,
        subscription: Subscription,
    ) -> anyhow::Result<Box<dyn DataReceiver>> {
        tracing::info!("qqq create data receiver");
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
