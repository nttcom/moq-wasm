use async_trait::async_trait;

use crate::modules::core::{
    data_receiver::receiver::DataReceiver,
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
    ) -> anyhow::Result<Box<dyn Subscription>>;
    async fn create_data_receiver(
        &mut self,
        subscription: &mut dyn Subscription,
    ) -> anyhow::Result<DataReceiver>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> Subscriber for moqt::Subscriber<T> {
    async fn _send_subscribe_namespace(&self, namespace: String) -> anyhow::Result<()> {
        self.subscribe_namespace(namespace).await
    }

    async fn send_subscribe(
        &self,
        track_namespace: String,
        track_name: String,
        option: SubscribeOption,
    ) -> anyhow::Result<Box<dyn Subscription>> {
        let option = moqt::SubscribeOption {
            subscriber_priority: option.subscriber_priority,
            group_order: option.group_order.as_moqt(),
            forward: option.forward,
            filter_type: option.filter_type.as_moqt(),
        };
        let subscription = self.subscribe(track_namespace, track_name, option).await?;
        Ok(Box::new(subscription))
    }

    async fn create_data_receiver(
        &mut self,
        subscription: &mut dyn Subscription,
    ) -> anyhow::Result<DataReceiver> {
        let Some(subscription) = subscription
            .as_any_mut()
            .downcast_mut::<moqt::Subscription<T>>()
        else {
            return Err(anyhow::anyhow!("subscription transport type mismatch"));
        };
        let result = self.accept_data_receiver(subscription).await?;
        match result {
            moqt::DataReceiver::Stream(inner) => {
                Ok(DataReceiver::Stream(Box::new(inner)))
            }
            moqt::DataReceiver::Datagram(inner) => {
                Ok(DataReceiver::Datagram(Box::new(inner)))
            }
        }
    }
}
