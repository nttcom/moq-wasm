use async_trait::async_trait;

use crate::modules::core::{handler::publish::SubscribeOption, subscription::Subscription};

#[async_trait]
pub(crate) trait Subscriber: 'static + Send + Sync {
    async fn send_subscribe_namespace(&self, namespaces: String) -> anyhow::Result<()>;
    async fn send_subscribe(
        &self,
        track_namespace: String,
        track_name: String,
        option: SubscribeOption,
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
        option: SubscribeOption,
    ) -> anyhow::Result<Box<dyn Subscription>> {
        let option = moqt::SubscribeOption {
            subscriber_priority: option.subscriber_priority,
            group_order: option.group_order.into_moqt(),
            forward: option.forward,
            filter_type: option.filter_type.into_moqt(),
            start_location: None,
            end_group: option.end_group,
        };
        let subscription = self.subscribe(track_namespace, track_name, option).await?;
        Ok(Box::new(subscription))
    }
}
