use std::sync::Arc;

use anyhow::bail;
use tracing::Span;

use crate::Publisher;
use crate::Subscriber;
use crate::modules::moqt::control_plane::enums::SessionEvent;
use crate::modules::moqt::data_plane::streams::stream::stream_receiver::BiStreamReceiver;
use crate::modules::moqt::domains::session_context::SessionContext;
use crate::modules::moqt::protocol::TransportProtocol;
use crate::modules::moqt::runtime::tasks::{
    control_message_receive_task::ControlMessageReceiveTask,
    datagram_receive_task::DatagramReceiveTask, disconnect_watch_task::DisconnectWatchTask,
    uni_stream_receive_task::UniStreamReceiveTask,
};

pub struct Session<T: TransportProtocol> {
    inner: Arc<SessionContext<T>>,
    session_span: Span,
    event_receiver: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<SessionEvent<T>>>,
    control_message_receive_task: tokio::task::JoinHandle<()>,
    datagram_receive_task: tokio::task::JoinHandle<()>,
    uni_stream_receive_task: tokio::task::JoinHandle<()>,
    disconnect_watch_task: tokio::task::JoinHandle<()>,
}

impl<T: TransportProtocol> Session<T> {
    pub(crate) fn new(
        receive_stream: BiStreamReceiver<T>,
        inner: SessionContext<T>,
        event_receiver: tokio::sync::mpsc::UnboundedReceiver<SessionEvent<T>>,
    ) -> Self {
        let inner = Arc::new(inner);
        let parent_span = Span::current();
        let session_span = tracing::info_span!(parent: &parent_span, "moqt.session");
        let control_plane_receiver_span = tracing::info_span!(
            parent: &session_span,
            "control_plane.receiver"
        );
        let datagram_receiver_span =
            tracing::info_span!(parent: &session_span, "data_plane.datagram_receiver");
        let uni_stream_receiver_span =
            tracing::info_span!(parent: &session_span, "data_plane.uni_stream_receiver");
        let transport_close_watcher_span =
            tracing::info_span!(parent: &session_span, "transport.close_watcher");

        let control_message_receive_task = ControlMessageReceiveTask::run(
            receive_stream,
            Arc::downgrade(&inner),
            control_plane_receiver_span,
        );
        let datagram_receive_task = DatagramReceiveTask::run(inner.clone(), datagram_receiver_span);
        let uni_stream_receive_task =
            UniStreamReceiveTask::run(inner.clone(), uni_stream_receiver_span);
        let disconnect_watch_task =
            DisconnectWatchTask::run(inner.clone(), transport_close_watcher_span);

        Self {
            inner,
            session_span,
            event_receiver: tokio::sync::Mutex::new(event_receiver),
            control_message_receive_task,
            datagram_receive_task,
            uni_stream_receive_task,
            disconnect_watch_task,
        }
    }

    pub fn publisher(&self) -> Publisher<T> {
        Publisher::<T> {
            session: self.inner.clone(),
        }
    }

    pub fn subscriber(&self) -> Subscriber<T> {
        Subscriber::<T> {
            session: self.inner.clone(),
        }
    }

    pub fn publisher_subscriber_pair(&self) -> (Publisher<T>, Subscriber<T>) {
        (self.publisher(), self.subscriber())
    }

    pub async fn receive_event(&self) -> anyhow::Result<SessionEvent<T>> {
        match self.event_receiver.lock().await.recv().await {
            Some(v) => Ok(v),
            None => bail!("Sender dropped."),
        }
    }
}

impl<T: TransportProtocol> Drop for Session<T> {
    fn drop(&mut self) {
        self.session_span.in_scope(|| {
            tracing::info!("Session dropped.");
        });
        self.control_message_receive_task.abort();
        self.datagram_receive_task.abort();
        self.uni_stream_receive_task.abort();
        self.disconnect_watch_task.abort();
    }
}
