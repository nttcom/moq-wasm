use std::sync::Arc;

use crate::Publisher;
use crate::Subscriber;
use crate::modules::moqt::controls::control_message_dispatcher::ControlMessageDispatcher;
use crate::modules::moqt::enums::SessionEvent;
use crate::modules::moqt::protocol::TransportProtocol;
use crate::modules::moqt::sessions::inner_session::InnerSession;

pub struct Session<T: TransportProtocol> {
    inner: Arc<InnerSession<T>>,
    event_receiver: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<SessionEvent>>,
    message_receive_join_handle: tokio::task::JoinHandle<()>,
}

impl<T: TransportProtocol> Session<T> {
    pub(crate) fn new(
        inner: InnerSession<T>,
        event_receiver: tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
    ) -> Self {
        let inner = Arc::new(inner);
        let message_receive_join_handle = ControlMessageDispatcher::run(Arc::downgrade(&inner));
        Self {
            inner,
            event_receiver: tokio::sync::Mutex::new(event_receiver),
            message_receive_join_handle,
        }
    }

    pub fn create_publisher(&self) -> Publisher<T> {
        Publisher::<T> {
            session: self.inner.clone(),
        }
    }

    pub fn create_subscriber(&self) -> Subscriber<T> {
        Subscriber::<T> {
            session: self.inner.clone(),
        }
    }

    pub fn create_publisher_subscriber_pair(&self) -> (Publisher<T>, Subscriber<T>) {
        (self.create_publisher(), self.create_subscriber())
    }

    pub async fn receive_event(&self) -> anyhow::Result<SessionEvent> {
        match self.event_receiver.lock().await.recv().await {
            Some(v) => Ok(v),
            None => todo!(),
        }
    }
}

impl<T: TransportProtocol> Drop for Session<T> {
    fn drop(&mut self) {
        tracing::info!("Session has been dropped.");
        self.message_receive_join_handle.abort();
    }
}
