mod modules;
use bytes::BytesMut;
use modules::buffer_manager;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use wtransport::RecvStream;
use wtransport::SendStream;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Ok, Result};
use tracing::{self, Instrument};
use tracing_subscriber::{self, filter::LevelFilter, EnvFilter};
use wtransport::{endpoint::IncomingSession, Endpoint, Identity, ServerConfig};

use crate::modules::buffer_manager::buffer_manager;
use crate::modules::buffer_manager::BufferCommand;
use crate::modules::track_manager::TrackCommand;

pub use moqt_core::constants;

use moqt_core::{constants::UnderlayType, message_handler::*, MOQTClient};

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
        // For track management but this is meaningless for now because transferring Object message is not implemented yet
        let (track_tx, mut track_rx) = mpsc::channel::<TrackCommand>(1024);

        if self.underlay != UnderlayType::WebTransport {
            bail!("Underlay must be WebTransport, not {:?}", self.underlay);
        }

        // Start buffer management thread
        tokio::spawn(async move { buffer_manager(&mut buffer_rx).await });

        // Start track management thread
        tokio::spawn(async move { modules::track_manager::track_manager(&mut track_rx).await });

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
            let incoming_session = server.accept().await;
            // Create a thread for each session
            tokio::spawn(
                handle_connection(buffer_tx, track_tx, incoming_session)
                    .instrument(tracing::info_span!("Connection", id)),
            );
        }

        Ok(())
    }
}

async fn handle_connection(
    buffer_tx: mpsc::Sender<BufferCommand>,
    track_tx: mpsc::Sender<TrackCommand>,
    incoming_session: IncomingSession,
) {
    let result = handle_connection_impl(buffer_tx, track_tx, incoming_session).await;
    tracing::error!("{:?}", result);
}

async fn handle_connection_impl(
    buffer_tx: mpsc::Sender<BufferCommand>,
    track_tx: mpsc::Sender<TrackCommand>,
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

                let (write_stream, read_stream) = stream;
                let stream_id = read_stream.id().into_u64();

                let buffer_tx = buffer_tx.clone();
                let track_tx = track_tx.clone();
                let close_tx = close_tx.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    let mut stream = Stream {
                        stream_type: StreamType::Bi,
                        stable_id,
                        stream_id,
                        read_stream,
                        write_stream,
                    };
                    handle_stream(&mut stream, client, buffer_tx, track_tx, close_tx).await
                });
            },
            stream = connection.accept_uni() => {
                let _span = tracing::info_span!("sid", stable_id); // TODO: 未実装

                let stream = stream?;
                tracing::info!("Accepted UNI stream");

                let read_stream = stream;
                let write_stream = connection.open_uni().await?.await?;
                let stream_id = read_stream.id().into_u64();

                let buffer_tx = buffer_tx.clone();
                let track_tx = track_tx.clone();
                let close_tx = close_tx.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    let mut stream = Stream {
                        stream_type: StreamType::Uni,
                        stable_id,
                        stream_id,
                        read_stream,
                        write_stream,
                    };
                    handle_stream(&mut stream, client, buffer_tx, track_tx, close_tx).await
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
    write_stream: SendStream,
}

async fn handle_stream(
    stream: &mut Stream,
    client: Arc<Mutex<MOQTClient>>,
    buffer_tx: Sender<BufferCommand>,
    track_tx: Sender<TrackCommand>,
    close_tx: Sender<(u64, String)>,
) -> Result<()> {
    let mut buffer = vec![0; 65536].into_boxed_slice();

    let stream_type = stream.stream_type;
    let stable_id = stream.stable_id;
    let stream_id = stream.stream_id;
    let read_stream = &mut stream.read_stream;
    let write_stream = &mut stream.write_stream;

    let mut track_manager = modules::track_manager::TrackManager::new(track_tx.clone());

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
        )
        .await;

        match message_result {
            MessageProcessResult::Success(buf) => {
                write_stream.write_all(&buf).await?;
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
