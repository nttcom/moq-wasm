use super::senders::{SenderToOtherConnectionThread, SendersToManagementThread};
use crate::modules::{
    buffer_manager::BufferCommand,
    moqt_client::MOQTClient,
    object_cache_storage::ObjectCacheStorageWrapper,
    pubsub_relation_manager::wrapper::PubSubRelationManagerWrapper,
    send_stream_dispatcher::SendStreamDispatchCommand,
    server_processes::{
        senders::{SenderToSelf, Senders},
        thread_starters::select_spawn_thread,
    },
};
use anyhow::Result;
use moqt_core::{
    data_stream_type::DataStreamType,
    pubsub_relation_manager_repository::PubSubRelationManagerRepository,
};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{self};
use wtransport::{endpoint::IncomingSession, Connection};

pub(crate) struct SessionHandler {
    session: Connection,
    client: Arc<Mutex<MOQTClient>>,
    close_session_rx: mpsc::Receiver<(u64, String)>,
    open_downstream_stream_or_datagram_rx: mpsc::Receiver<(u64, DataStreamType)>,
}

impl SessionHandler {
    pub(crate) async fn start(
        senders_to_other_connection_thread: SenderToOtherConnectionThread,
        senders_to_management_thread: SendersToManagementThread,
        incoming_session: IncomingSession,
    ) -> Result<()> {
        let mut session_handler = Self::init(
            senders_to_other_connection_thread,
            senders_to_management_thread,
            incoming_session,
        )
        .await?;

        session_handler.main_loop().await?;

        session_handler.terminate().await?;

        Ok(())
    }

    async fn init(
        senders_to_other_connection_thread: SenderToOtherConnectionThread,
        senders_to_management_thread: SendersToManagementThread,
        incoming_session: IncomingSession,
    ) -> Result<Self> {
        let session_request = incoming_session.await?;
        tracing::info!(
            "New session: Authority: '{}', Path: '{}'",
            session_request.authority(),
            session_request.path()
        );

        let session = session_request.accept().await?;
        let stable_id = session.stable_id();

        let session_span = tracing::info_span!("Session", stable_id);
        session_span.in_scope(|| {
            tracing::info!("Waiting for data from client...");
        });

        let (close_session_tx, close_session_rx) = mpsc::channel::<(u64, String)>(32);

        let senders_to_self = SenderToSelf::new(close_session_tx);
        let senders = Senders::new(
            senders_to_self,
            senders_to_other_connection_thread,
            senders_to_management_thread,
        );

        // For opening a new data stream
        let (open_downstream_stream_or_datagram_tx, open_downstream_stream_or_datagram_rx) =
            mpsc::channel::<(u64, DataStreamType)>(32);
        senders
            .open_downstream_stream_or_datagram_txes()
            .lock()
            .await
            .insert(stable_id, open_downstream_stream_or_datagram_tx);

        let client = Arc::new(Mutex::new(MOQTClient::new(stable_id, senders)));

        let session_handler = SessionHandler {
            session,
            client,
            close_session_rx,
            open_downstream_stream_or_datagram_rx,
        };

        Ok(session_handler)
    }

    async fn main_loop(&mut self) -> Result<()> {
        let mut is_control_stream_opened = false;

        loop {
            match select_spawn_thread(
                &self.client,
                &self.session,
                &mut self.open_downstream_stream_or_datagram_rx,
                &mut self.close_session_rx,
                &mut is_control_stream_opened,
            )
            .await
            {
                Ok(_) => {}
                Err(err) => {
                    tracing::error!("Main loop broken: {:?}", err);
                    break;
                }
            };
        }

        Ok(())
    }

    async fn terminate(&mut self) -> Result<()> {
        let senders = self.client.lock().await.senders();
        let stable_id = self.client.lock().await.id();

        // Delete pub/sub information related to the client
        let pubsub_relation_manager =
            PubSubRelationManagerWrapper::new(senders.pubsub_relation_tx().clone());
        let _ = pubsub_relation_manager.delete_client(stable_id).await;

        // Delete object cache related to the client
        // FIXME: It should not be deleted if the cache should be stored
        //   (Now, it is deleted immediately because to clean up cpu and memory)
        let mut object_cache_storage =
            ObjectCacheStorageWrapper::new(senders.object_cache_tx().clone());
        let _ = object_cache_storage.delete_client(stable_id).await;

        // Delete senders to the client
        senders
            .send_stream_tx()
            .send(SendStreamDispatchCommand::Delete {
                session_id: stable_id,
            })
            .await?;

        // FIXME: Do not remove if storing QUIC-level sessions
        senders
            .buffer_tx()
            .send(BufferCommand::ReleaseSession {
                session_id: stable_id,
            })
            .await?;

        tracing::info!("session terminated");

        Ok(())
    }
}
