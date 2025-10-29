use async_trait::async_trait;

use crate::modules::core::datagram_receiver::DatagramReceiver;

pub(crate) enum Acceptance {
    Datagram(Box<dyn DatagramReceiver>, moqt::DatagramObject),
}

pub struct Subscription {
    pub track_alias: u64,
    pub expires: u64,
    pub is_group_order_acsending: bool,
    pub content_exists: bool,
    pub start_location_group_id: Option<u64>,
    pub start_location_object_id: Option<u64>,
}

#[async_trait]
pub(crate) trait Subscriber: 'static + Send + Sync {
    async fn send_subscribe_namespace(&self, namespaces: String) -> anyhow::Result<()>;
    async fn send_subscribe(
        &self,
        track_namespace: String,
        track_name: String,
    ) -> anyhow::Result<Subscription>;
    async fn accept_stream_or_datagram(&self, track_alias: u64) -> anyhow::Result<Acceptance>;
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
    ) -> anyhow::Result<Subscription> {
        let result = self
            .subscribe(
                track_namespace,
                track_name,
                moqt::SubscribeOption::default(),
            )
            .await?;
        Ok(Subscription {
            track_alias: result.track_alias,
            expires: result.expires,
            is_group_order_acsending: result.group_order == moqt::GroupOrder::Ascending,
            content_exists: result.content_exists,
            start_location_group_id: result.start_location_group_id,
            start_location_object_id: result.start_location_object_id,
        })
    }

    async fn accept_stream_or_datagram(&self, track_alias: u64) -> anyhow::Result<Acceptance> {
        let result = self.accept_stream_or_datagram(track_alias).await?;
        match result {
            moqt::Acceptance::Stream(stream) => todo!(),
            moqt::Acceptance::Datagram(datagram, object) => {
                Ok(Acceptance::Datagram(Box::new(datagram), object))
            }
        }
    }
}
