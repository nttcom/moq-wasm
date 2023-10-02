mod modules;
use bytes::BytesMut;
use modules::buffer_manager;
use modules::constants::UnderlayType;
use modules::message_handler::*;
use modules::moqt_client;
use modules::moqt_client::MOQTClient;
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
use wtransport::{endpoint::IncomingSession, tls::Certificate, Endpoint, ServerConfig};

use crate::modules::buffer_manager::buffer_manager;
use crate::modules::buffer_manager::BufferCommand;

pub use modules::constants;

pub enum AuthCallbackType {
    Announce,
    Subscribe,
}

pub struct MOQTConfig {
    pub port: u16,
    pub cert_path: String,
    pub key_path: String,
    pub keep_alive_interval_sec: u64,
    pub underlay: UnderlayType,
    pub auth_callback:
        Option<fn(track_name: String, auth_payload: String, AuthCallbackType) -> Result<()>>,
}

impl MOQTConfig {
    pub fn new() -> MOQTConfig {
        MOQTConfig {
            port: 4433,
            cert_path: "./cert.pem".to_string(),
            key_path: "./key.pem".to_string(),
            keep_alive_interval_sec: 3,
            underlay: UnderlayType::Both,
            auth_callback: None,
        }
    }
}

pub struct MOQT {
    port: u16,
    cert_path: String,
    key_path: String,
    keep_alive_interval_sec: u64,
    underlay: UnderlayType,
    auth_callback:
        Option<fn(track_name: String, auth_payload: String, AuthCallbackType) -> Result<()>>,
}

impl MOQT {
    pub fn new(config: MOQTConfig) -> MOQT {
        MOQT {
            port: config.port,
            cert_path: config.cert_path,
            key_path: config.key_path,
            keep_alive_interval_sec: config.keep_alive_interval_sec,
            underlay: config.underlay,
            auth_callback: config.auth_callback,
        }
    }
    pub async fn start(&self) -> Result<()> {
        init_logging();

        let (tx, mut rx) = mpsc::channel::<BufferCommand>(1024);

        if let UnderlayType::WebTransport = self.underlay {
        } else {
            bail!("Underlay must be WebTransport, not {:?}", self.underlay)
        }

        tokio::spawn(async move { buffer_manager(&mut rx).await });

        // 以下はWebTransportの場合
        let config = ServerConfig::builder()
            .with_bind_default(self.port)
            .with_certificate(
                Certificate::load(&self.cert_path, &self.key_path).with_context(|| {
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
            let tx = tx.clone();
            let incoming_session = server.accept().await;
            tokio::spawn(
                handle_connection(tx, incoming_session)
                    .instrument(tracing::info_span!("Connection", id)),
            );
        }

        Ok(())
    }
}

async fn handle_connection(tx: mpsc::Sender<BufferCommand>, incoming_session: IncomingSession) {
    let result = handle_connection_impl(tx, incoming_session).await;
    tracing::error!("{:?}", result);
}

async fn handle_connection_impl(
    tx: mpsc::Sender<BufferCommand>,
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

    loop {
        tokio::select! {
            stream = connection.accept_bi() => {
                let span = tracing::info_span!("sid", stable_id);

                let stream = stream?;

                span.in_scope(|| {
                    tracing::info!("Accepted BI stream");
                });

                let stream_id = stream.1.id().into_u64();

                let (mut write_stream, mut read_stream) = stream;

                let tx = tx.clone();
                let close_tx = close_tx.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    handle_stream(StreamType::Bi,client, stable_id, stream_id, &mut read_stream, &mut write_stream, tx, close_tx).await
                });
            },
            stream = connection.accept_uni() => {
                let span = tracing::info_span!("sid", stable_id);

                let stream = stream?;
                tracing::info!("Accepted UNI stream");

                let stream_id = stream.id().into_u64();

                let mut read_stream = stream;
                let mut write_stream = connection.open_uni().await?.await?;

                let tx = tx.clone();
                let close_tx = close_tx.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    handle_stream(StreamType::Uni,client, stable_id, stream_id, &mut read_stream, &mut write_stream, tx, close_tx).await
                });
            },
            _ = connection.closed() => {
                tracing::info!("Connection closed, rtt={:?}", connection.rtt());
                break;
            },
            Some((code, reason)) = close_rx.recv() => {
                tracing::info!("close channel received");
                // FIXME: closeしたいけどVarIntがexportされていないのでお茶を濁す
                // wtransport-protoにあるかも?
                // connection.close(VarInt)
                break;
            }
        }
    }

    tx.send(BufferCommand::ReleaseSession { session_id: stable_id }).await?;

    Ok(())
}

async fn handle_stream(
    stream_type: StreamType,
    client: Arc<Mutex<MOQTClient>>,
    stable_id: usize,
    stream_id: u64,
    read_stream: &mut RecvStream,
    write_stream: &mut SendStream,
    tx: Sender<BufferCommand>,
    close_tx: Sender<(u64, String)>,
) -> Result<()> {
    let mut buffer = vec![0; 65536].into_boxed_slice();

    loop {
        let span = tracing::info_span!("sid", stable_id);
        let bytes_read = match read_stream.read(&mut buffer).instrument(span).await? {
            Some(bytes_read) => bytes_read,
            None => break,
        };

        tracing::info!("bytes_read: {}", bytes_read);

        let tx = tx.clone();
        let read_buf = BytesMut::from(&buffer[..bytes_read]);
        let buf = buffer_manager::request_buffer(tx, stable_id, stream_id).await;
        let mut buf = buf.lock().await;
        buf.extend_from_slice(&read_buf.to_vec());

        let mut client = client.lock().await;
        let message_result = message_handler(
            &mut buf,
            stream_type.clone(),
            UnderlayType::WebTransport,
            &mut client,
        );

        match message_result {
            MessageProcessResult::Success(buf) => {
                write_stream.write_all(&mut buf.to_vec()).await?;
                tracing::info!("sent {:?}", buf.to_vec());
            }
            MessageProcessResult::Failure(code, message) => {
                close_tx.send((u8::from(code) as u64, message)).await?;
                break;
            }
            MessageProcessResult::Fragment => (),
        };
    }

    tx.send(BufferCommand::ReleaseStream { session_id: stable_id, stream_id }).await?;

    Ok::<()>(())
}

fn init_logging() {
    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();

    tracing_subscriber::fmt()
        .with_target(true)
        .with_level(true)
        .with_env_filter(env_filter)
        .init();
}
