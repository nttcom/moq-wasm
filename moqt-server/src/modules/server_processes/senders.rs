use crate::{
    modules::{
        buffer_manager::BufferCommand, control_message_dispatcher::ControlMessageDispatchCommand,
        object_cache_storage::commands::ObjectCacheStorageCommand,
        pubsub_relation_manager::commands::PubSubRelationCommand,
        signal_dispatcher::SignalDispatchCommand,
    },
    SenderToOpenSubscription,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{mpsc, Mutex};

#[derive(Debug)]
pub(crate) struct SenderToSelf {
    close_session_tx: mpsc::Sender<(u64, String)>,
}

impl SenderToSelf {
    pub(crate) fn new(close_session_tx: mpsc::Sender<(u64, String)>) -> Self {
        SenderToSelf { close_session_tx }
    }
}

#[derive(Debug)]
pub(crate) struct SenderToOtherConnectionThread {
    start_forwarder_txes: Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>>,
}

impl SenderToOtherConnectionThread {
    pub(crate) fn new(
        start_forwarder_txes: Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>>,
    ) -> Self {
        SenderToOtherConnectionThread {
            start_forwarder_txes,
        }
    }
}

#[derive(Debug)]
pub(crate) struct SendersToManagementThread {
    buffer_tx: mpsc::Sender<BufferCommand>,
    pubsub_relation_tx: mpsc::Sender<PubSubRelationCommand>,
    control_message_dispatch_tx: mpsc::Sender<ControlMessageDispatchCommand>,
    signal_dispatch_tx: mpsc::Sender<SignalDispatchCommand>,
    object_cache_tx: mpsc::Sender<ObjectCacheStorageCommand>,
}

impl SendersToManagementThread {
    pub(crate) fn new(
        buffer_tx: mpsc::Sender<BufferCommand>,
        pubsub_relation_tx: mpsc::Sender<PubSubRelationCommand>,
        control_message_dispatch_tx: mpsc::Sender<ControlMessageDispatchCommand>,
        signal_dispatch_tx: mpsc::Sender<SignalDispatchCommand>,
        object_cache_tx: mpsc::Sender<ObjectCacheStorageCommand>,
    ) -> Self {
        SendersToManagementThread {
            buffer_tx,
            pubsub_relation_tx,
            control_message_dispatch_tx,
            signal_dispatch_tx,
            object_cache_tx,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Senders {
    sender_to_self: SenderToSelf,
    sender_to_other_connection_thread: SenderToOtherConnectionThread,
    senders_to_management_thread: SendersToManagementThread,
}

impl Senders {
    pub fn new(
        sender_to_self: SenderToSelf,
        sender_to_other_connection_thread: SenderToOtherConnectionThread,
        senders_to_management_thread: SendersToManagementThread,
    ) -> Self {
        Senders {
            sender_to_self,
            sender_to_other_connection_thread,
            senders_to_management_thread,
        }
    }

    pub(crate) fn start_forwarder_txes(
        &self,
    ) -> &Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>> {
        &self.sender_to_other_connection_thread.start_forwarder_txes
    }

    pub(crate) fn close_session_tx(&self) -> &mpsc::Sender<(u64, String)> {
        &self.sender_to_self.close_session_tx
    }

    pub(crate) fn buffer_tx(&self) -> &mpsc::Sender<BufferCommand> {
        &self.senders_to_management_thread.buffer_tx
    }

    pub(crate) fn pubsub_relation_tx(&self) -> &mpsc::Sender<PubSubRelationCommand> {
        &self.senders_to_management_thread.pubsub_relation_tx
    }

    pub(crate) fn control_message_dispatch_tx(
        &self,
    ) -> &mpsc::Sender<ControlMessageDispatchCommand> {
        &self
            .senders_to_management_thread
            .control_message_dispatch_tx
    }

    pub(crate) fn signal_dispatch_tx(&self) -> &mpsc::Sender<SignalDispatchCommand> {
        &self.senders_to_management_thread.signal_dispatch_tx
    }

    pub(crate) fn object_cache_tx(&self) -> &mpsc::Sender<ObjectCacheStorageCommand> {
        &self.senders_to_management_thread.object_cache_tx
    }
}

#[cfg(test)]
pub(crate) mod test_helper_fn {
    use super::{SenderToSelf, Senders};
    use std::{collections::HashMap, sync::Arc};
    use tokio::sync::Mutex;

    pub(crate) fn create_senders_mock() -> Senders {
        let (close_session_tx, _) = tokio::sync::mpsc::channel(1);
        let sender_to_self = SenderToSelf::new(close_session_tx);

        let start_forwarder_txes = Arc::new(Mutex::new(HashMap::new()));
        let sender_to_other_connection_thread =
            super::SenderToOtherConnectionThread::new(start_forwarder_txes);

        let (buffer_tx, _) = tokio::sync::mpsc::channel(1);
        let (pubsub_relation_tx, _) = tokio::sync::mpsc::channel(1);
        let (control_message_dispatch_tx, _) = tokio::sync::mpsc::channel(1);
        let (signal_dispatch_tx, _) = tokio::sync::mpsc::channel(1);
        let (object_cache_tx, _) = tokio::sync::mpsc::channel(1);
        let senders_to_management_thread = super::SendersToManagementThread::new(
            buffer_tx,
            pubsub_relation_tx,
            control_message_dispatch_tx,
            signal_dispatch_tx,
            object_cache_tx,
        );

        Senders::new(
            sender_to_self,
            sender_to_other_connection_thread,
            senders_to_management_thread,
        )
    }
}
