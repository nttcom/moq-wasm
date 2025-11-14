use std::collections::HashSet;
use std::sync::Arc;

use anyhow::bail;

use crate::Publisher;
use crate::StreamReceiver;
use crate::Subscriber;
use crate::modules::moqt::enums::SessionEvent;
use crate::modules::moqt::protocol::TransportProtocol;
use crate::modules::moqt::sessions::session_context::SessionContext;
use crate::modules::moqt::threads::control_message_receive_thread::ControlMessageReceiveThread;
use crate::modules::moqt::threads::datagram_receive_thread::DatagramReceiveThread;

pub struct Session<T: TransportProtocol> {
    inner: Arc<SessionContext<T>>,
    event_receiver: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<SessionEvent<T>>>,
    message_receive_join_handle: tokio::task::JoinHandle<()>,
    datagram_receive_thread: tokio::task::JoinHandle<()>,
}

impl<T: TransportProtocol> Session<T> {
    pub(crate) fn new(
        receive_stream: StreamReceiver<T>,
        inner: SessionContext<T>,
        event_receiver: tokio::sync::mpsc::UnboundedReceiver<SessionEvent<T>>,
    ) -> Self {
        let inner = Arc::new(inner);
        let message_receive_join_handle =
            ControlMessageReceiveThread::run(receive_stream, Arc::downgrade(&inner));
        let datagram_receive_thread = DatagramReceiveThread::run(inner.clone());

        Self {
            inner,
            event_receiver: tokio::sync::Mutex::new(event_receiver),
            message_receive_join_handle,
            datagram_receive_thread,
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

    pub async fn get_subscribed_namespaces(&self) -> HashSet<String> {
        let subscribed_namespaces = self.inner.subscribed_namespaces.lock().await;
        subscribed_namespaces.clone()
    }

    pub async fn receive_event(&self) -> anyhow::Result<SessionEvent<T>> {
        match self.event_receiver.lock().await.recv().await {
            Some(v) => Ok(v),
            None => bail!("Sender has been dropped."),
        }
    }
}

impl<T: TransportProtocol> Drop for Session<T> {
    fn drop(&mut self) {
        tracing::info!("Session has been dropped.");
        self.message_receive_join_handle.abort();
        self.datagram_receive_thread.abort();
    }
}
