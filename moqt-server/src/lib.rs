mod modules;
use crate::modules::buffer_manager::{buffer_manager, BufferCommand};
use crate::modules::relay_handler_manager::{relay_handler_manager, RelayHandlerCommand};
use crate::modules::track_namespace_manager::{track_namespace_manager, TrackCommand};
use anyhow::{bail, Context, Ok, Result};
use bytes::BytesMut;
use modules::buffer_manager;
pub use moqt_core::constants;
use moqt_core::constants::TerminationErrorCode;
use moqt_core::message_type::MessageType;
use moqt_core::messages::moqt_payload::MOQTPayload;
use moqt_core::messages::object_message::{ObjectWithLength, ObjectWithoutLength};
use moqt_core::messages::subscribe_error_message::SubscribeError;
use moqt_core::messages::subscribe_ok_message::SubscribeOk;
use moqt_core::messages::subscribe_request_message::SubscribeRequestMessage;
use moqt_core::variable_integer::write_variable_integer;
use moqt_core::{constants::UnderlayType, message_handler::*, MOQTClient};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::Mutex;
use tracing::{self, Instrument};
use tracing_subscriber::{self, filter::LevelFilter, EnvFilter};
use wtransport::RecvStream;
use wtransport::SendStream;
use wtransport::{endpoint::IncomingSession, Endpoint, Identity, ServerConfig};

// Callback to validate the Auth parameter
pub enum AuthCallbackType {
    Announce,
    Subscribe,
}

pub type AuthCallbackFunctionType =
    fn(track_name: String, auth_payload: String, AuthCallbackType) -> Result<()>;

pub struct MOQTConfig {
    pub port: u16,
    pub cert_path: String,
    pub key_path: String,
    pub keep_alive_interval_sec: u64,
    pub underlay: UnderlayType,
    pub auth_callback: Option<AuthCallbackFunctionType>,
    pub log_level: String,
}

impl Default for MOQTConfig {
    fn default() -> Self {
        MOQTConfig::new()
    }
}

impl MOQTConfig {
    // TODO: use getter/setter
    pub fn new() -> MOQTConfig {
        MOQTConfig {
            port: 4433,
            cert_path: "./cert.pem".to_string(),
            key_path: "./key.pem".to_string(),
            keep_alive_interval_sec: 3,
            underlay: UnderlayType::Both,
            auth_callback: None,
            log_level: "DEBUG".to_string(),
        }
    }
}

pub struct MOQT {
    port: u16,
    cert_path: String,
    key_path: String,
    keep_alive_interval_sec: u64,
    underlay: UnderlayType,
    // auth_callback: Option<AuthCallbackFunctionType>,
    log_level: String,
}

impl MOQT {
    pub fn new(config: MOQTConfig) -> MOQT {
        MOQT {
            port: config.port,
            cert_path: config.cert_path,
            key_path: config.key_path,
            keep_alive_interval_sec: config.keep_alive_interval_sec,
            underlay: config.underlay,
            // auth_callback: config.auth_callback,
            log_level: config.log_level,
        }
    }
    pub async fn start(&self) -> Result<()> {
        init_logging(self.log_level.to_string());

        // For buffer management for each stream
        let (buffer_tx, mut buffer_rx) = mpsc::channel::<BufferCommand>(1024);
        // For track management
        let (track_namespace_tx, mut track_namespace_rx) = mpsc::channel::<TrackCommand>(1024);
        // For relay handler management
        let (relay_handler_tx, mut relay_handler_rx) = mpsc::channel::<RelayHandlerCommand>(1024);

        if self.underlay != UnderlayType::WebTransport {
            bail!("Underlay must be WebTransport, not {:?}", self.underlay);
        }

        // Start buffer management thread
        tokio::spawn(async move { buffer_manager(&mut buffer_rx).await });

        // Start track management thread
        tokio::spawn(async move { track_namespace_manager(&mut track_namespace_rx).await });

        // Start stream management thread
        tokio::spawn(async move { relay_handler_manager(&mut relay_handler_rx).await });

        // Start wtransport server
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

        for id in 0.. {
            let buffer_tx = buffer_tx.clone();
            let track_namespace_tx = track_namespace_tx.clone();
            let relay_handler_tx = relay_handler_tx.clone();
            let incoming_session = server.accept().await;
            // Create a thread for each session
            tokio::spawn(
                handle_connection(
                    buffer_tx,
                    track_namespace_tx,
                    relay_handler_tx,
                    incoming_session,
                )
                .instrument(tracing::info_span!("Connection", id)),
            );
        }

        Ok(())
    }
}

async fn handle_connection(
    buffer_tx: mpsc::Sender<BufferCommand>,
    track_namespace_tx: mpsc::Sender<TrackCommand>,
    relay_handler_tx: mpsc::Sender<RelayHandlerCommand>,
    incoming_session: IncomingSession,
) {
    let result = handle_connection_impl(
        buffer_tx,
        track_namespace_tx,
        relay_handler_tx,
        incoming_session,
    )
    .await;
    tracing::error!("{:?}", result);
}

async fn handle_connection_impl(
    buffer_tx: mpsc::Sender<BufferCommand>,
    track_namespace_tx: mpsc::Sender<TrackCommand>,
    relay_handler_tx: mpsc::Sender<RelayHandlerCommand>,
    incoming_session: IncomingSession,
) -> Result<()> {
    tracing::info!("Waiting for session request...");

    let session_request = incoming_session.await?;

    tracing::info!(
        "New session: Authority: '{}', Path: '{}'",
        session_request.authority(),
        session_request.path()
    );

    let connection = session_request.accept().await?;
    let stable_id = connection.stable_id();

    let span = tracing::info_span!("sid", stable_id);
    let _guard = span.enter();

    let client = Arc::new(Mutex::new(MOQTClient::new(stable_id)));

    tracing::info!("Waiting for data from client...");

    let (close_tx, mut close_rx) = mpsc::channel::<(u64, String)>(32);

    let (uni_relay_tx, mut uni_relay_rx) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
    relay_handler_tx
        .send(RelayHandlerCommand::Set {
            session_id: stable_id,
            stream_type: "unidirectional_stream".to_string(),
            sender: uni_relay_tx,
        })
        .await?;

    // TODO: FIXME: Need to store information between threads for QUIC-level reconnection support
    let mut is_control_stream_opened = false;

    loop {
        tokio::select! {
            // Waiting for a bi-directional stream and processing the received message
            stream = connection.accept_bi() => {
                if is_control_stream_opened {
                    // Only 1 control stream is allowed
                    tracing::info!("Control stream already opened");
                    close_tx.send((u8::from(constants::TerminationErrorCode::ProtocolViolation) as u64, "Control stream already opened".to_string())).await?;
                    break;
                }
                is_control_stream_opened = true;

                let span = tracing::info_span!("sid", stable_id);

                let stream = stream?;

                span.in_scope(|| {
                    tracing::info!("Accepted BI stream");
                });

                // The write_stream is wrapped with a Mutex to make it thread-safe since it can be called from multiple threads for returning and relaying messages.
                let (write_stream, read_stream) = stream;
                let shread_write_stream = Arc::new(Mutex::new(write_stream));

                let stream_id = read_stream.id().into_u64();

                let buffer_tx = buffer_tx.clone();
                let track_namespace_tx = track_namespace_tx.clone();
                let relay_handler_tx = relay_handler_tx.clone();
                let close_tx = close_tx.clone();
                let client= client.clone();

                let (message_tx, message_rx) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
                relay_handler_tx.send(RelayHandlerCommand::Set {
                    session_id: stable_id,
                    stream_type: "bidirectional_stream".to_string(),
                    sender: message_tx,
                }).await?;

                // Thread that listens for WebTransport messages
                let write_stream = Arc::clone(&shread_write_stream);
                tokio::spawn(async move {
                    let mut stream = BiStream {
                        stable_id,
                        stream_id,
                        read_stream,
                        shread_write_stream: write_stream,
                    };
                    handle_incoming_bi_stream(&mut stream, client, buffer_tx, track_namespace_tx, close_tx, relay_handler_tx).await
                });

                // Thread to send relayed messages (ANNOUNCE SUBSCRIBE) from the server
                let write_stream = Arc::clone(&shread_write_stream);
                tokio::spawn(async move {
                    handle_bi_relay(write_stream, message_rx).await;
                });
            },
            // Waiting for a uni-directional recv stream and processing the received message
            stream = connection.accept_uni() => {
                let read_stream = stream.unwrap();

                let span = tracing::info_span!("sid", stable_id); // TODO: Not implemented yet
                span.in_scope(|| {
                    tracing::info!("Accepted UNI stream");
                });
                let stream_id = read_stream.id().into_u64();

                let buffer_tx = buffer_tx.clone();
                let track_namespace_tx = track_namespace_tx.clone();
                let relay_handler_tx = relay_handler_tx.clone();
                let close_tx = close_tx.clone();
                let client = client.clone();

                let mut stream = UniRecvStream {
                    stable_id,
                    stream_id,
                    read_stream,
                };
                let _ = handle_incoming_uni_stream(&mut stream, client, buffer_tx, track_namespace_tx, close_tx, relay_handler_tx).await;

            },
            // Waiting for a uni-directional relay request and relaying the message
            Some(message) = uni_relay_rx.recv() => {
                // A sender MUST send each object over a dedicated stream.
                let write_stream = connection.open_uni().await?.await?;
                // Send relayed messages (OBJECT) from the server
                handle_uni_relay(write_stream, message).await;
            },
            _ = connection.closed() => {
                tracing::info!("Connection closed, rtt={:?}", connection.rtt());
                break;
            },
            // TODO: Not implemented yet
            Some((_code, _reason)) = close_rx.recv() => {
                tracing::info!("close channel received");
                // FIXME: I want to close the connection, but VarInt is not exported, so I'll leave it as is
                // Maybe it's in wtransport-proto?
                // connection.close(VarInt)
                break;
            }
        }
    }

    // FIXME: Do not remove if storing QUIC-level sessions
    buffer_tx
        .send(BufferCommand::ReleaseSession {
            session_id: stable_id,
        })
        .await?;

    Ok(())
}

struct UniRecvStream {
    stable_id: usize,
    stream_id: u64,
    read_stream: RecvStream,
}

async fn handle_incoming_uni_stream(
    stream: &mut UniRecvStream,
    client: Arc<Mutex<MOQTClient>>,
    buffer_tx: Sender<BufferCommand>,
    track_namespace_tx: Sender<TrackCommand>,
    close_tx: Sender<(u64, String)>,
    relay_handler_tx: Sender<RelayHandlerCommand>,
) -> Result<()> {
    let mut buffer = vec![0; 65536].into_boxed_slice();

    let stable_id = stream.stable_id;
    let stream_id = stream.stream_id;
    let read_stream = &mut stream.read_stream;

    let mut track_namespace_manager =
        modules::track_namespace_manager::TrackNamespaceManager::new(track_namespace_tx.clone());
    let mut relay_handler_manager =
        modules::relay_handler_manager::RelayHandlerManager::new(relay_handler_tx.clone());

    let span = tracing::info_span!("sid", stable_id);
    let bytes_read = match read_stream.read(&mut buffer).instrument(span).await? {
        Some(bytes_read) => bytes_read,
        None => return Err(anyhow::anyhow!("Failed to read from stream")),
    };

    tracing::info!("bytes_read: {}", bytes_read);

    let read_buf = BytesMut::from(&buffer[..bytes_read]);
    let buf = buffer_manager::request_buffer(buffer_tx.clone(), stable_id, stream_id).await;
    let mut buf = buf.lock().await;
    buf.extend_from_slice(&read_buf);

    let mut client = client.lock().await;
    // TODO: Move the implementation of message_handler to the server side since it is only used by the server
    let message_result = message_handler(
        &mut buf,
        StreamType::Uni,
        UnderlayType::WebTransport,
        &mut client,
        &mut track_namespace_manager,
        &mut relay_handler_manager,
    )
    .await;

    match message_result {
        MessageProcessResult::SuccessWithoutResponse => {
            tracing::info!("SuccessWithoutResponse");
        }
        MessageProcessResult::Failure(code, message) => {
            close_tx
                .send((u8::from(code) as u64, message.clone()))
                .await?;
            return Err(anyhow::anyhow!(message));
        }
        MessageProcessResult::Fragment => (),
        MessageProcessResult::Success(_) => {
            let message = "Unsuported message type for uni-directional stream".to_string();
            close_tx
                .send((
                    u8::from(TerminationErrorCode::GenericError) as u64,
                    message.clone(),
                ))
                .await?;
            return Err(anyhow::anyhow!(message));
        }
    };

    buffer_tx
        .send(BufferCommand::ReleaseStream {
            session_id: stable_id,
            stream_id,
        })
        .await?;

    Ok::<()>(())
}

struct BiStream {
    stable_id: usize,
    stream_id: u64,
    read_stream: RecvStream,
    shread_write_stream: Arc<Mutex<SendStream>>,
}

async fn handle_incoming_bi_stream(
    stream: &mut BiStream,
    client: Arc<Mutex<MOQTClient>>,
    buffer_tx: Sender<BufferCommand>,
    track_namespace_tx: Sender<TrackCommand>,
    close_tx: Sender<(u64, String)>,
    relay_handler_tx: Sender<RelayHandlerCommand>,
) -> Result<()> {
    let mut buffer = vec![0; 65536].into_boxed_slice();

    let stable_id = stream.stable_id;
    let stream_id = stream.stream_id;
    let read_stream = &mut stream.read_stream;
    let shread_write_stream = &mut stream.shread_write_stream;

    let mut track_namespace_manager =
        modules::track_namespace_manager::TrackNamespaceManager::new(track_namespace_tx.clone());
    let mut relay_handler_manager =
        modules::relay_handler_manager::RelayHandlerManager::new(relay_handler_tx.clone());

    loop {
        let span = tracing::info_span!("sid", stable_id);
        let bytes_read = match read_stream.read(&mut buffer).instrument(span).await? {
            Some(bytes_read) => bytes_read,
            None => break,
        };

        tracing::info!("bytes_read: {}", bytes_read);

        let read_buf = BytesMut::from(&buffer[..bytes_read]);
        let buf = buffer_manager::request_buffer(buffer_tx.clone(), stable_id, stream_id).await;
        let mut buf = buf.lock().await;
        buf.extend_from_slice(&read_buf);

        let mut client = client.lock().await;
        // TODO: Move the implementation of message_handler to the server side since it is only used by the server
        let message_result = message_handler(
            &mut buf,
            StreamType::Bi,
            UnderlayType::WebTransport,
            &mut client,
            &mut track_namespace_manager,
            &mut relay_handler_manager,
        )
        .await;

        match message_result {
            MessageProcessResult::Success(buf) => {
                let mut shread_write_stream = shread_write_stream.lock().await;
                shread_write_stream.write_all(&buf).await?;
                tracing::info!("sent {:x?}", buf.to_vec());
            }
            MessageProcessResult::SuccessWithoutResponse => {
                tracing::info!("SuccessWithoutResponse");
            }
            MessageProcessResult::Failure(code, message) => {
                close_tx.send((u8::from(code) as u64, message)).await?;
                break;
            }
            MessageProcessResult::Fragment => (),
        };
    }

    buffer_tx
        .send(BufferCommand::ReleaseStream {
            session_id: stable_id,
            stream_id,
        })
        .await?;

    Ok::<()>(())
}

async fn handle_uni_relay(mut write_stream: SendStream, message: Arc<Box<dyn MOQTPayload>>) {
    let mut write_buf = BytesMut::new();
    message.packetize(&mut write_buf);
    let mut message_buf = BytesMut::with_capacity(write_buf.len() + 8);

    if message
        .as_any()
        .downcast_ref::<ObjectWithLength>()
        .is_some()
    {
        message_buf.extend(write_variable_integer(
            u8::from(MessageType::ObjectWithLength) as u64,
        ));
    } else if message
        .as_any()
        .downcast_ref::<ObjectWithoutLength>()
        .is_some()
    {
        message_buf.extend(write_variable_integer(
            u8::from(MessageType::ObjectWithoutLength) as u64,
        ));
    } else {
        tracing::error!("Unsupported message type for uni-directional stream");
        return;
    }

    message_buf.extend(write_buf);

    if let Err(e) = write_stream.write_all(&message_buf).await {
        tracing::error!("Failed to write to stream: {:?}", e);
        return;
    }
    tracing::info!("message relayed: {:?}", message_buf.to_vec());
    let _ = write_stream.finish().await;
}

async fn handle_bi_relay(
    write_stream: Arc<Mutex<SendStream>>,
    mut message_rx: Receiver<Arc<Box<dyn MOQTPayload>>>,
) {
    while let Some(message) = message_rx.recv().await {
        let mut write_buf = BytesMut::new();
        message.packetize(&mut write_buf);
        let mut message_buf = BytesMut::with_capacity(write_buf.len() + 8);

        if message
            .as_any()
            .downcast_ref::<SubscribeRequestMessage>()
            .is_some()
        {
            message_buf.extend(write_variable_integer(
                u8::from(MessageType::Subscribe) as u64
            ));
        } else if message.as_any().downcast_ref::<SubscribeOk>().is_some() {
            message_buf.extend(write_variable_integer(
                u8::from(MessageType::SubscribeOk) as u64
            ));
        } else if message.as_any().downcast_ref::<SubscribeError>().is_some() {
            message_buf.extend(write_variable_integer(
                u8::from(MessageType::SubscribeError) as u64,
            ));
        } else {
            tracing::error!("Unsupported message type for bi-directional stream");
            continue;
        }

        message_buf.extend(write_buf);
        tracing::info!("message relayed: {:x?}", message_buf.to_vec());

        let mut shread_write_stream = write_stream.lock().await;
        if let Err(e) = shread_write_stream.write_all(&message_buf).await {
            tracing::error!("Failed to write to stream: {:?}", e);
            break;
        }
    }
}

fn init_logging(log_level: String) {
    let level_filter: LevelFilter = match log_level.to_uppercase().as_str() {
        "OFF" => LevelFilter::OFF,
        "TRACE" => LevelFilter::TRACE,
        "DEBUG" => LevelFilter::DEBUG,
        "INFO" => LevelFilter::INFO,
        "WARN" => LevelFilter::WARN,
        "ERROR" => LevelFilter::ERROR,
        _ => {
            panic!(
                "Invalid log level: '{}'.\n  Valid log levels: [OFF, TRACE, DEBUG, INFO, WARN, ERROR]",
                log_level
            );
        }
    };

    let env_filter = EnvFilter::builder()
        .with_default_directive(level_filter.into())
        .from_env_lossy();

    tracing_subscriber::fmt()
        .with_target(true)
        .with_level(true)
        .with_env_filter(env_filter)
        .init();

    tracing::info!("Logging initialized. (Level: {})", log_level);
}
