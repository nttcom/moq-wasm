use std::sync::Arc;

use tracing::Span;

use crate::modules::{
    executor::AbortOnDrop,
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
    _control_message_receive_task: AbortOnDrop,
    _datagram_receive_task: AbortOnDrop,
    _uni_stream_receive_task: AbortOnDrop,
    _disconnect_watch_task: AbortOnDrop,
    _publish_done_expiry_task: AbortOnDrop,
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
            _control_message_receive_task: control_receiver
                .spawn(
                    Arc::downgrade(session),
                    Arc::downgrade(connection),
                    publish_done_sender,
                    session_span.clone(),
                )
                .abort_on_drop(),
            _datagram_receive_task: DatagramReceiveTask::run(
                connection.clone(),
                session_span.clone(),
            )
            .abort_on_drop(),
            _uni_stream_receive_task: UniStreamReceiveTask::run(
                connection.clone(),
                session_span.clone(),
            )
            .abort_on_drop(),
            _disconnect_watch_task: DisconnectWatchTask::run(
                connection.clone(),
                connection.event_sender.clone(),
                session_span.clone(),
            )
            .abort_on_drop(),
            _publish_done_expiry_task: PublishDoneExpiryTask::run(
                publish_done_receiver,
                Arc::downgrade(session),
                Arc::downgrade(connection),
                session_span.clone(),
            )
            .abort_on_drop(),
        }
    }
}
