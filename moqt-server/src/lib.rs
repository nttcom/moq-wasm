use anyhow::{bail, Context, Result};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{mpsc, mpsc::Sender, Mutex};
use tokio::task;
use tracing::{self, Instrument};
use wtransport::quinn::TransportConfig;
use wtransport::{Endpoint, Identity, ServerConfig};
mod modules;
pub use modules::config::MOQTConfig;
use modules::{
    buffer_manager::{buffer_manager, BufferCommand},
    control_message_dispatcher::{control_message_dispatcher, ControlMessageDispatchCommand},
    logging::init_logging,
    object_cache_storage::{
        cache::SubgroupStreamId, commands::ObjectCacheStorageCommand, storage::object_cache_storage,
    },
    pubsub_relation_manager::{commands::PubSubRelationCommand, manager::pubsub_relation_manager},
    server_processes::{
        senders::{SenderToOtherConnectionThread, SendersToManagementThread},
        session_handler::SessionHandler,
    },
    signal_dispatcher,
};
pub use moqt_core::constants;
use moqt_core::{
    constants::{TerminationErrorCode, UnderlayType},
    data_stream_type::DataStreamType,
};

use crate::signal_dispatcher::{signal_dispatcher, SignalDispatchCommand};

type SubscribeId = u64;
pub(crate) type SenderToOpenSubscription =
    Sender<(SubscribeId, DataStreamType, Option<SubgroupStreamId>)>;
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
        init_logging(self.log_level.to_string());

        if self.underlay != UnderlayType::WebTransport {
            bail!("Underlay must be WebTransport, not {:?}", self.underlay);
        }
        let mut transport_config = TransportConfig::default();
        // 単方向ストリーム数を100000に設定
        transport_config.max_concurrent_uni_streams(100000u32.into());
        // transport_config.time_threshold(1.5);
        // transport_config.packet_threshold(5);
        // transport_config.stream_receive_window(VarInt::from_u32(10 * 1024 * 1024)); // initial_max_stream_data_uniと同義。デフォルトは65,536 バイト (64KB)なので1MBにする
        let config = ServerConfig::builder()
            .with_bind_default(self.port)
            .with_custom_transport(
                Identity::load_pemfiles(&self.cert_path, &self.key_path)
                    .await
                    .with_context(|| {
                        format!(
                            "cert load failed. '{}' or '{}' not found.",
                            self.cert_path, self.key_path
                        )
                    })?,
                transport_config,
            )
            .keep_alive_interval(Some(Duration::from_secs(self.keep_alive_interval_sec)))
            .build();
        // let _ = config.quic_endpoint_config_mut().max_udp_payload_size(1350);

        let server = Endpoint::server(config)?;
        tracing::info!("Server ready!");

        // Spawn management thread
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

        for id in 0.. {
            let sender_to_other_connection_thread =
                SenderToOtherConnectionThread::new(start_forwarder_txes.clone());
            let senders_to_management_thread = SendersToManagementThread::new(
                buffer_tx.clone(),
                pubsub_relation_tx.clone(),
                control_message_dispatch_tx.clone(),
                signal_dispatch_tx.clone(),
                object_cache_tx.clone(),
            );

            let incoming_session = server.accept().await;
            let session_span = tracing::info_span!("Session", id);

            // Create a thread for each session
            task::Builder::new()
                .name("WT Session Handler")
                .spawn(async move {
                    let mut session_handler = SessionHandler::init(
                        sender_to_other_connection_thread,
                        senders_to_management_thread,
                        incoming_session,
                    )
                    .instrument(session_span.clone())
                    .await
                    .unwrap();

                    match session_handler
                        .start()
                        .instrument(session_span.clone())
                        .await
                    {
                        Ok(_) => {}
                        Err(err) => {
                            tracing::error!("{:#?}", err);
                        }
                    }

                    let _ = session_handler.finish().instrument(session_span).await;
                })?;
        }

        Ok(())
    }
}
