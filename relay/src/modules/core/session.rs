use async_trait::async_trait;

use crate::modules::core::{
    publisher::Publisher, session_event::MoqtSessionEvent, subscriber::Subscriber,
};

#[async_trait]
pub(crate) trait Session: 'static + Send + Sync {
    fn as_publisher(&self) -> Box<dyn Publisher>;
    fn as_subscriber(&self) -> Box<dyn Subscriber>;
    async fn receive_moqt_session_event(&self) -> anyhow::Result<MoqtSessionEvent>;
}

#[async_trait]
impl<T: moqt::TransportProtocol> Session for moqt::Session<T> {
    fn as_publisher(&self) -> Box<dyn Publisher> {
        Box::new(self.publisher())
    }

    fn as_subscriber(&self) -> Box<dyn Subscriber> {
        Box::new(self.subscriber())
    }

    async fn receive_moqt_session_event(&self) -> anyhow::Result<MoqtSessionEvent> {
        let event = self.receive_event().await?;
        let result = match event {
            moqt::SessionEvent::PublishNamespace(publish_namespace_handler) => {
                MoqtSessionEvent::PublishNamespace(Box::new(publish_namespace_handler))
            }
            moqt::SessionEvent::SubscribeNameSpace(subscribe_namespace_handler) => {
                MoqtSessionEvent::SubscribeNamespace(Box::new(subscribe_namespace_handler))
            }
            moqt::SessionEvent::Publish(publish_handler) => {
                MoqtSessionEvent::Publish(Box::new(publish_handler))
            }
            moqt::SessionEvent::Subscribe(subscribe_handler) => {
                MoqtSessionEvent::Subscribe(Box::new(subscribe_handler))
            }
            moqt::SessionEvent::Unsubscribe(unsubscribe_handler) => {
                MoqtSessionEvent::Unsubscribe(Box::new(unsubscribe_handler))
            }
            moqt::SessionEvent::Disconnected() => MoqtSessionEvent::Disconnected(),
            moqt::SessionEvent::ProtocolViolation() => MoqtSessionEvent::ProtocolViolation(),
        };
        Ok(result)
    }
}
