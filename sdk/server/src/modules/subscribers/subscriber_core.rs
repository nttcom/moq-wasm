use async_trait::async_trait;

#[async_trait]
pub(crate) trait SubscriberCore {
    async fn recv_from_publisher(&mut self) -> anyhow::Result<moqt::PublisherEvent>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> SubscriberCore for moqt::Subscriber<T>  {
    async fn recv_from_publisher(&mut self) -> anyhow::Result<moqt::PublisherEvent> {
        self.receive_from_publisher().await
    }
}