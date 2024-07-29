mod modules;
use crate::modules::buffer_manager::{buffer_manager, BufferCommand};
use crate::modules::stream_manager::{stream_manager, StreamCommand};
use crate::modules::track_manager::{track_manager, TrackCommand};
use anyhow::{bail, Context, Ok, Result};
use bytes::BytesMut;
use modules::buffer_manager;
pub use moqt_core::constants;
use moqt_core::message_type::MessageType;
use moqt_core::messages::moqt_payload::MOQTPayload;
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

// Auth paramを検証するためのcallback
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
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);
        // For stream management
        let (stream_tx, mut stream_rx) = mpsc::channel::<StreamCommand>(1024);

        if self.underlay != UnderlayType::WebTransport {
            bail!("Underlay must be WebTransport, not {:?}", self.underlay);
        }

        // Start buffer management thread
        tokio::spawn(async move { buffer_manager(&mut buffer_rx).await });

        // Start track management thread
        tokio::spawn(async move { track_manager(&mut track_rx).await });

        // Start stream management thread
        tokio::spawn(async move { stream_manager(&mut stream_rx).await });

        // 以下はWebTransportの場合
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
            let track_tx = track_tx.clone();
            let stream_tx = stream_tx.clone();
            let incoming_session = server.accept().await;
            // Create a thread for each session
            tokio::spawn(
                handle_connection(buffer_tx, track_tx, stream_tx, incoming_session)
                    .instrument(tracing::info_span!("Connection", id)),
            );
        }

        Ok(())
    }
}

async fn handle_connection(
    buffer_tx: mpsc::Sender<BufferCommand>,
    track_tx: mpsc::Sender<TrackCommand>,
    stream_tx: mpsc::Sender<StreamCommand>,
    incoming_session: IncomingSession,
) {
    let result = handle_connection_impl(buffer_tx, track_tx, stream_tx, incoming_session).await;
    tracing::error!("{:?}", result);
}

async fn handle_connection_impl(
    buffer_tx: mpsc::Sender<BufferCommand>,
    track_tx: mpsc::Sender<TrackCommand>,
    stream_tx: mpsc::Sender<StreamCommand>,
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

    // Close処理をまとめるためにチャネルで処理を送る
    let (close_tx, mut close_rx) = mpsc::channel::<(u64, String)>(32);

    // TODO: FIXME: QUICレベルの再接続対応のためにはスレッド間で記憶する必要がある
    let mut is_control_stream_opened = false;
    loop {
        tokio::select! {
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

                // write_steamはMessageの返却・中継のため、複数スレッドから呼び出されることがあるためMutexでラップしてスレッドセーフにする
                let (write_stream, read_stream) = stream;
                let shread_write_stream = Arc::new(Mutex::new(write_stream));


                let stream_id = read_stream.id().into_u64();

                let buffer_tx = buffer_tx.clone();
                let track_tx = track_tx.clone();
                let stream_tx = stream_tx.clone();
                let close_tx = close_tx.clone();
                let client = client.clone();

                let (message_tx, message_rx) = mpsc::channel::<Arc<Box<dyn MOQTPayload>>>(1024);
                stream_tx.send(StreamCommand::Set {
                    session_id: stable_id,
                    stream_type: "bidirectional_stream".to_string(),
                    sender: message_tx,
                }).await?;

                // WebTrasnportのメッセージを待ち受けるスレッド
                let write_stream_clone = Arc::clone(&shread_write_stream);
                tokio::spawn(async move {
                    let mut stream = Stream {
                        stream_type: StreamType::Bi,
                        stable_id,
                        stream_id,
                        read_stream,
                        shread_write_stream: write_stream_clone,
                    };
                    handle_read_stream(&mut stream, client, buffer_tx, track_tx, close_tx, stream_tx).await
                });

                // サーバーが中継するメッセージ(ANNOUNCE SUBSCRIBE OBJECT)を送信するスレッド
                let write_stream_clone = Arc::clone(&shread_write_stream);
                tokio::spawn(async move {
                    handle_relayed_message(write_stream_clone, message_rx).await;
                });

            },
            stream = connection.accept_uni() => {
                let _span = tracing::info_span!("sid", stable_id); // TODO: 未実装

                let stream = stream?;
                tracing::info!("Accepted UNI stream");

                let read_stream = stream;
                let write_stream = connection.open_uni().await?.await?;
                let shread_write_stream = Arc::new(Mutex::new(write_stream));
                let stream_id = read_stream.id().into_u64();

                let buffer_tx = buffer_tx.clone();
                let track_tx = track_tx.clone();
                let close_tx = close_tx.clone();
                let client = client.clone();
                let stream_tx_clone = stream_tx.clone();
                tokio::spawn(async move {
                    let mut stream = Stream {
                        stream_type: StreamType::Uni,
                        stable_id,
                        stream_id,
                        read_stream,
                        shread_write_stream,
                    };
                    handle_read_stream(&mut stream, client, buffer_tx, track_tx, close_tx, stream_tx_clone).await
                });
            },
            _ = connection.closed() => {
                tracing::info!("Connection closed, rtt={:?}", connection.rtt());
                break;
            },
            // TODO: 未実装のため＿をつけている
            Some((_code, _reason)) = close_rx.recv() => {
                tracing::info!("close channel received");
                // FIXME: closeしたいけどVarIntがexportされていないのでお茶を濁す
                // wtransport-protoにあるかも?
                // connection.close(VarInt)
                break;
            }
        }
    }

    // FIXME: QUICレベルのsessionを保持する場合は消してはいけない
    buffer_tx
        .send(BufferCommand::ReleaseSession {
            session_id: stable_id,
        })
        .await?;

    Ok(())
}

struct Stream {
    stream_type: StreamType,
    stable_id: usize,
    stream_id: u64,
    read_stream: RecvStream,
    shread_write_stream: Arc<Mutex<SendStream>>,
}

async fn handle_read_stream(
    stream: &mut Stream,
    client: Arc<Mutex<MOQTClient>>,
    buffer_tx: Sender<BufferCommand>,
    track_tx: Sender<TrackCommand>,
    close_tx: Sender<(u64, String)>,
    stream_tx: Sender<StreamCommand>,
) -> Result<()> {
    let mut buffer = vec![0; 65536].into_boxed_slice();

    let stream_type = stream.stream_type;
    let stable_id = stream.stable_id;
    let stream_id = stream.stream_id;
    let read_stream = &mut stream.read_stream;
    let shread_write_stream = &mut stream.shread_write_stream;

    let mut track_manager = modules::track_manager::TrackManager::new(track_tx.clone());
    let mut stream_manager = modules::stream_manager::StreamManager::new(stream_tx.clone());

    loop {
        let span = tracing::info_span!("sid", stable_id);
        let bytes_read = match read_stream.read(&mut buffer).instrument(span).await? {
            Some(bytes_read) => bytes_read,
            None => break,
        };

        tracing::info!("bytes_read: {}", bytes_read);

        let buffer_tx = buffer_tx.clone();
        let read_buf = BytesMut::from(&buffer[..bytes_read]);
        let buf = buffer_manager::request_buffer(buffer_tx, stable_id, stream_id).await;
        let mut buf = buf.lock().await;
        buf.extend_from_slice(&read_buf);

        let mut client = client.lock().await;
        // TODO: message_handlerはserverでしか使わないのでserver側に実装を移動する
        let message_result = message_handler(
            &mut buf,
            stream_type,
            UnderlayType::WebTransport,
            &mut client,
            &mut track_manager,
            &mut stream_manager,
        )
        .await;

        match message_result {
            MessageProcessResult::Success(buf) => {
                let mut shread_write_stream = shread_write_stream.lock().await;
                shread_write_stream.write_all(&buf).await?;
                tracing::info!("sent {:x?}", buf.to_vec());
            }
            MessageProcessResult::Failure(code, message) => {
                close_tx.send((u8::from(code) as u64, message)).await?;
                break;
            }
            MessageProcessResult::Fragment => (),
        };
    }

    // TODO: FIXME: QUICレベルの再接続時をサポートする時に呼んで良いか確認
    buffer_tx
        .send(BufferCommand::ReleaseStream {
            session_id: stable_id,
            stream_id,
        })
        .await?;

    Ok::<()>(())
}

async fn handle_relayed_message(
    write_stream_clone: Arc<Mutex<SendStream>>,
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
            tracing::info!("message is SubscribeRequestMessage");
            message_buf.extend(write_variable_integer(
                u8::from(MessageType::Subscribe) as u64
            ));
        } else if message.as_any().downcast_ref::<SubscribeOk>().is_some() {
            tracing::info!("message is SubscribeOkMessage");
            message_buf.extend(write_variable_integer(
                u8::from(MessageType::SubscribeOk) as u64
            ));
        } else if message.as_any().downcast_ref::<SubscribeError>().is_some() {
            tracing::info!("message is SubscribeErrorMessage");
            message_buf.extend(write_variable_integer(
                u8::from(MessageType::SubscribeError) as u64,
            ));
        } else {
            tracing::info!("message is UNKOWN Message");
            message_buf.extend(write_variable_integer(
                u8::from(MessageType::Announce) as u64
            ));
        }

        message_buf.extend(write_buf);
        tracing::info!("message relayed: {:?}", message_buf);

        let mut shread_write_stream = write_stream_clone.lock().await;
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
