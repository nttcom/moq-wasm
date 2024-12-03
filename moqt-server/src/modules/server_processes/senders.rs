use crate::{
    modules::{
        buffer_manager::BufferCommand, object_cache_storage::ObjectCacheStorageCommand,
        pubsub_relation_manager::commands::PubSubRelationCommand,
        send_stream_dispatcher::SendStreamDispatchCommand,
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
    open_downstream_stream_or_datagram_txes: Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>>,
}

impl SenderToOtherConnectionThread {
    pub(crate) fn new(
        open_downstream_stream_or_datagram_txes: Arc<
            Mutex<HashMap<usize, SenderToOpenSubscription>>,
        >,
    ) -> Self {
        SenderToOtherConnectionThread {
            open_downstream_stream_or_datagram_txes,
        }
    }
}

#[derive(Debug)]
pub(crate) struct SendersToManagementThread {
    buffer_tx: mpsc::Sender<BufferCommand>,
    pubsub_relation_tx: mpsc::Sender<PubSubRelationCommand>,
    send_stream_tx: mpsc::Sender<SendStreamDispatchCommand>,
    object_cache_tx: mpsc::Sender<ObjectCacheStorageCommand>,
}

impl SendersToManagementThread {
    pub(crate) fn new(
        buffer_tx: mpsc::Sender<BufferCommand>,
        pubsub_relation_tx: mpsc::Sender<PubSubRelationCommand>,
        send_stream_tx: mpsc::Sender<SendStreamDispatchCommand>,
        object_cache_tx: mpsc::Sender<ObjectCacheStorageCommand>,
    ) -> Self {
        SendersToManagementThread {
            buffer_tx,
            pubsub_relation_tx,
            send_stream_tx,
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

    pub(crate) fn open_downstream_stream_or_datagram_txes(
        &self,
    ) -> &Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>> {
        &self
            .sender_to_other_connection_thread
            .open_downstream_stream_or_datagram_txes
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

    pub(crate) fn send_stream_tx(&self) -> &mpsc::Sender<SendStreamDispatchCommand> {
        &self.senders_to_management_thread.send_stream_tx
    }

    pub(crate) fn object_cache_tx(&self) -> &mpsc::Sender<ObjectCacheStorageCommand> {
        &self.senders_to_management_thread.object_cache_tx
    }
}
