use std::{collections::HashMap, sync::Arc};

use tokio::{sync::Mutex, sync::mpsc, task};

use crate::{
    SenderToOpenSubscription,
    modules::{
        buffer_manager::{BufferCommand, buffer_manager},
        control_message_dispatcher::{ControlMessageDispatchCommand, control_message_dispatcher},
        object_cache_storage::{
            commands::ObjectCacheStorageCommand, storage::object_cache_storage,
        },
        pubsub_relation_manager::{
            commands::PubSubRelationCommand, manager::pubsub_relation_manager,
        },
        server_processes::senders::{SenderToOtherConnectionThread, SendersToManagementThread},
        signal_dispatcher::{SignalDispatchCommand, signal_dispatcher},
    },
};

struct ThreadsFactory;

impl ThreadsFactory {
    pub fn create() -> SendersToManagementThread {
        let (buffer_tx, mut buffer_rx) = mpsc::channel::<BufferCommand>(1024);
        task::Builder::new()
            .name("Buffer Manager")
            .spawn(async move { buffer_manager(&mut buffer_rx).await })?;
        let (pubsub_relation_tx, mut pubsub_relation_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        task::Builder::new()
            .name("PubSub Relation Manager")
            .spawn(async move { pubsub_relation_manager(&mut pubsub_relation_rx).await })?;
        let (control_message_dispatch_tx, mut control_message_dispatch_rx) =
            mpsc::channel::<ControlMessageDispatchCommand>(1024);
        task::Builder::new()
            .name("Control Message Dispatcher")
            .spawn(
                async move { control_message_dispatcher(&mut control_message_dispatch_rx).await },
            )?;
        let (signal_dispatch_tx, mut signal_dispatch_rx) =
            mpsc::channel::<SignalDispatchCommand>(1024);
        task::Builder::new()
            .name("Signal Dispatcher")
            .spawn(async move { signal_dispatcher(&mut signal_dispatch_rx).await })?;

        let (object_cache_tx, mut object_cache_rx) =
            mpsc::channel::<ObjectCacheStorageCommand>(1024);
        task::Builder::new()
            .name("Object Cache Storage")
            .spawn(async move { object_cache_storage(&mut object_cache_rx).await })?;

        let start_forwarder_txes: Arc<Mutex<HashMap<usize, SenderToOpenSubscription>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let sender_to_other_connection_thread =
            SenderToOtherConnectionThread::new(start_forwarder_txes.clone());
        SendersToManagementThread::new(
            buffer_tx.clone(),
            pubsub_relation_tx.clone(),
            control_message_dispatch_tx.clone(),
            signal_dispatch_tx.clone(),
            object_cache_tx.clone(),
        )
    }
}
