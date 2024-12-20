use anyhow::{bail, Context, Result};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{mpsc, mpsc::Sender, Mutex};
use tracing::{self, Instrument};
use wtransport::{Endpoint, Identity, ServerConfig};
mod modules;
pub use modules::moqt_config::MOQTConfig;
use modules::{
    buffer_manager::{buffer_manager, BufferCommand},
    logging,
    object_cache_storage::{object_cache_storage, ObjectCacheStorageCommand},
    pubsub_relation_manager::{commands::PubSubRelationCommand, manager::pubsub_relation_manager},
    send_stream_dispatcher::{send_stream_dispatcher, SendStreamDispatchCommand},
    server_processes::{
        senders::{SenderToOtherConnectionThread, SendersToManagementThread},
        session_handler::SessionHandler,
    },
};
pub use moqt_core::constants;
use moqt_core::{
    constants::{TerminationErrorCode, UnderlayType},
    data_stream_type::DataStreamType,
};

type SubscribeId = u64;
pub(crate) type SenderToOpenSubscription = Sender<(SubscribeId, DataStreamType)>;
pub(crate) type TerminationError = (TerminationErrorCode, String);

pub struct MOQTServer {
    port: u16,
    cert_path: String,
    key_path: String,
    keep_alive_interval_sec: u64,
    underlay: UnderlayType,
    log_level: String,
}

impl MOQTServer {
    pub fn new(config: MOQTConfig) -> MOQTServer {
        MOQTServer {
            port: config.port,
            cert_path: config.cert_path,
            key_path: config.key_path,
            keep_alive_interval_sec: config.keep_alive_interval_sec,
            underlay: config.underlay,
            log_level: config.log_level,
        }
    }
    pub async fn start(&self) -> Result<()> {
        logging::init_logging(self.log_level.to_string());

        if self.underlay != UnderlayType::WebTransport {
            bail!("Underlay must be WebTransport, not {:?}", self.underlay);
        }
        let config = ServerConfig::builder()
            .with_bind_default(self.port)
            .with_identity(
                &Identity::load_pemfiles(&self.cert_path, &self.key_path)
                    .await
                    .with_context(|| {
                        format!(
                            "cert load failed. '{}' or '{}' not found.",
                            self.cert_path, self.key_path
                        )
                    })?,
            )
            .keep_alive_interval(Some(Duration::from_secs(self.keep_alive_interval_sec)))
            .build();
        let server = Endpoint::server(config)?;
        tracing::info!("Server ready!");

        // Spawn management thread
        let (buffer_tx, mut buffer_rx) = mpsc::channel::<BufferCommand>(1024);
        tokio::spawn(async move { buffer_manager(&mut buffer_rx).await });
        let (pubsub_relation_tx, mut pubsub_relation_rx) =
            mpsc::channel::<PubSubRelationCommand>(1024);
        tokio::spawn(async move { pubsub_relation_manager(&mut pubsub_relation_rx).await });
        let (send_stream_tx, mut send_stream_rx) = mpsc::channel::<SendStreamDispatchCommand>(1024);
        tokio::spawn(async move { send_stream_dispatcher(&mut send_stream_rx).await });
        let (object_cache_tx, mut object_cache_rx) =
            mpsc::channel::<ObjectCacheStorageCommand>(1024);
        tokio::spawn(async move { object_cache_storage(&mut object_cache_rx).await });

        let open_downstream_stream_or_datagram_txes: Arc<
            Mutex<HashMap<usize, SenderToOpenSubscription>>,
        > = Arc::new(Mutex::new(HashMap::new()));

        for id in 0.. {
            let sender_to_other_connection_thread =
                SenderToOtherConnectionThread::new(open_downstream_stream_or_datagram_txes.clone());
            let senders_to_management_thread = SendersToManagementThread::new(
                buffer_tx.clone(),
                pubsub_relation_tx.clone(),
                send_stream_tx.clone(),
                object_cache_tx.clone(),
            );

            let incoming_session = server.accept().await;
            let session_span = tracing::info_span!("Session", id);

            // Create a thread for each session
            tokio::spawn(async move {
                let result = SessionHandler::start(
                    sender_to_other_connection_thread,
                    senders_to_management_thread,
                    incoming_session,
                )
                .instrument(session_span)
                .await;

                tracing::error!("{:?}", result);
            });
        }

        Ok(())
    }
}
