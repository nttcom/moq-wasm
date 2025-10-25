use async_trait::async_trait;

use crate::modules::core::datagram_sender::DatagramSender;

pub(crate) struct PublishResult {
    pub is_group_order_ascending: bool,
    pub subscriber_priority: u8,
    pub forward: bool,
    pub filter_type: moqt::FilterType,
    pub largest_location_group_id: Option<u64>,
    pub largest_location_object_id: Option<u64>,
    pub end_group: Option<u64>,
}

#[async_trait]
pub(crate) trait Publisher: 'static + Send + Sync {
    async fn send_publish_namespace(&self, namespaces: String) -> anyhow::Result<()>;
    async fn send_publish(
        &self,
        track_namespace: String,
        track_name: String,
        track_alias: u64,
    ) -> anyhow::Result<PublishResult>;
    fn create_datagram(&self, track_alias: u64) -> Box<dyn DatagramSender>;
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
    ) -> anyhow::Result<PublishResult> {
        let mut option = moqt::PublishOption::default();
        option.track_alias = track_alias;
        let result = self.publish(track_namespace, track_name, option).await?;
        let is_group_order_ascending = result.group_order == moqt::GroupOrder::Ascending;
        Ok(PublishResult {
            is_group_order_ascending,
            subscriber_priority: result.subscriber_priority,
            forward: result.forward,
            filter_type: result.filter_type,
            largest_location_group_id: result.largest_location_group_id,
            largest_location_object_id: result.largest_location_object_id,
            end_group: result.end_group,
        })
    }

    fn create_datagram(&self, track_alias: u64) -> Box<dyn DatagramSender> {
        let sender = self.create_datagram(track_alias);
        Box::new(sender)
    }
}
