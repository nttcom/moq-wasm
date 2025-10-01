use anyhow::bail;
use std::sync::Arc;

use crate::Publisher;
use crate::Subscriber;
use crate::modules::moqt::enums::ReceiveEvent;
use crate::modules::moqt::enums::SessionEvent;
use crate::modules::moqt::protocol::TransportProtocol;
use crate::modules::moqt::sessions::inner_session::InnerSession;
use crate::modules::moqt::sessions::session_message_resolver::SessionMessageResolver;

pub struct Session<T: TransportProtocol> {
    pub id: usize,
    inner: Arc<InnerSession<T>>,
}

impl<T: TransportProtocol> Session<T> {
    pub(crate) fn new(inner: InnerSession<T>) -> Self {
        let id = inner.id;
        Self {
            id,
            inner: Arc::new(inner),
        }
    }

    pub fn create_publisher(&self) -> Publisher<T> {
        Publisher::<T> {
            session: self.inner.clone(),
            shared_send_stream: self.inner.send_stream.clone(),
            event_sender: self.inner.event_sender.clone(),
        }
    }

    pub fn create_subscriber(&self) -> Subscriber<T> {
        Subscriber::<T> {
            session: self.inner.clone(),
            shared_send_stream: self.inner.send_stream.clone(),
            event_sender: self.inner.event_sender.clone(),
        }
    }

    pub async fn receive_event(&self) -> anyhow::Result<SessionEvent> {
        let mut receiver = self.inner.event_sender.subscribe();
        let receive_message = receiver.recv().await?;
        match receive_message {
            ReceiveEvent::Message(binary) => SessionMessageResolver::resolve_message(binary),
            ReceiveEvent::Error() => bail!("Error occurred."),
        }
    }
}
