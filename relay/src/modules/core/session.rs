use async_trait::async_trait;

use crate::modules::core::{
    publisher::Publisher, session_event::SessionEvent, subscriber::Subscriber,
};

#[async_trait]
pub(crate) trait Session: 'static + Send + Sync {
    fn new_publisher_subscriber_pair(&self) -> (Box<dyn Publisher>, Box<dyn Subscriber>);
    async fn receive_session_event(&self) -> anyhow::Result<SessionEvent>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> Session for moqt::Session<T> {
    fn new_publisher_subscriber_pair(&self) -> (Box<dyn Publisher>, Box<dyn Subscriber>) {
        let (publisher, subscriber) = self.create_publisher_subscriber_pair();
        (Box::new(publisher), Box::new(subscriber))
    }

    async fn receive_session_event(&self) -> anyhow::Result<SessionEvent> {
        let event = self.receive_event().await?;
        let result = match event {
            moqt::SessionEvent::PublishNamespace(publish_namespace_handler) => {
                SessionEvent::PublishNamespace(Box::new(publish_namespace_handler))
            }
            moqt::SessionEvent::SubscribeNameSpace(subscribe_namespace_handler) => {
                SessionEvent::SubscribeNamespace(Box::new(subscribe_namespace_handler))
            }
            moqt::SessionEvent::Publish(publish_handler) => {
                SessionEvent::Publish(Box::new(publish_handler))
            }
            moqt::SessionEvent::Subscribe(subscribe_handler) => {
                SessionEvent::Subscribe(Box::new(subscribe_handler))
            }
            moqt::SessionEvent::ProtocolViolation() => SessionEvent::ProtocolViolation(),
        };
        Ok(result)
    }
}
