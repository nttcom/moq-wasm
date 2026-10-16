use std::sync::Arc;

use tracing::Span;

use crate::modules::{
    executor::JoinHandle,
    moqt::{
        domains::{connection::Connection, session_context::SessionContext},
        runtime::tasks::{
            datagram_receive_task::DatagramReceiveTask, disconnect_watch_task::DisconnectWatchTask,
            publish_done_expiry_task::PublishDoneExpiryTask,
            uni_stream_receive_task::UniStreamReceiveTask,
        },
        version::ControlReceiver,
    },
};

pub(crate) struct ConnectionTasks {
    control_message_receive_task: JoinHandle,
    datagram_receive_task: JoinHandle,
    uni_stream_receive_task: JoinHandle,
    disconnect_watch_task: JoinHandle,
    publish_done_expiry_task: JoinHandle,
}

impl ConnectionTasks {
    pub(crate) fn run(
        session: &Arc<SessionContext>,
        connection: &Arc<Connection>,
        control_receiver: ControlReceiver,
        session_span: &Span,
    ) -> Self {
        let (publish_done_sender, publish_done_receiver) = tokio::sync::mpsc::unbounded_channel();
        Self {
            control_message_receive_task: control_receiver.spawn(
                Arc::downgrade(session),
                Arc::downgrade(connection),
                publish_done_sender,
                tracing::info_span!(parent: session_span, "moqt.control_plane.receiver"),
            ),
            datagram_receive_task: DatagramReceiveTask::run(
                connection.clone(),
                tracing::info_span!(parent: session_span, "data_plane.datagram_receiver"),
            ),
            uni_stream_receive_task: UniStreamReceiveTask::run(
                connection.clone(),
                tracing::info_span!(parent: session_span, "data_plane.uni_stream_receiver"),
            ),
            disconnect_watch_task: DisconnectWatchTask::run(
                connection.clone(),
                session.event_sender.clone(),
                tracing::info_span!(parent: session_span, "moqt.transport.close_watcher"),
            ),
            publish_done_expiry_task: PublishDoneExpiryTask::run(
                publish_done_receiver,
                Arc::downgrade(session),
                Arc::downgrade(connection),
                tracing::info_span!(parent: session_span, "moqt.publish_done_expiry"),
            ),
        }
    }
}

impl Drop for ConnectionTasks {
    fn drop(&mut self) {
        self.control_message_receive_task.abort();
        self.datagram_receive_task.abort();
        self.uni_stream_receive_task.abort();
        self.disconnect_watch_task.abort();
        self.publish_done_expiry_task.abort();
    }
}
