use async_trait::async_trait;

#[async_trait]
pub(crate) trait PublisherCore {
    async fn recv_from_subscriber(&mut self) -> anyhow::Result<moqt::SubscriberEvent>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> PublisherCore for moqt::Publisher<T>  {
    async fn recv_from_subscriber(&mut self) -> anyhow::Result<moqt::SubscriberEvent> {
        self.receive_from_subscriber().await
    }
}