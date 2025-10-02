use async_trait::async_trait;

use crate::modules::core::{publisher::Publisher, subscriber::Subscriber};

#[async_trait]
pub(crate) trait Session: 'static + Send + Sync {
    fn new_publisher_subscriber_pair(&self) -> (Box<dyn Publisher>, Box<dyn Subscriber>);
    async fn receive_session_event(&mut self) -> anyhow::Result<moqt::SessionEvent>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> Session for moqt::Session<T> {
    fn new_publisher_subscriber_pair(&self) -> (Box<dyn Publisher>, Box<dyn Subscriber>) {
        let (publisher, subscriber) = self.create_publisher_subscriber_pair();
        (Box::new(publisher), Box::new(subscriber))
    }

    async fn receive_session_event(&mut self) -> anyhow::Result<moqt::SessionEvent> {
        self.receive_event().await
    }
}
