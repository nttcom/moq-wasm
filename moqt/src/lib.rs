pub mod constants;
// mod modules;
use constants::UnderlayType;

use std::time::Duration;

use anyhow::{bail, Context, Ok, Result};
use tracing::{self, Instrument};
use tracing_subscriber::{self, filter::LevelFilter, EnvFilter};
use wtransport::{endpoint::IncomingSession, tls::Certificate, Endpoint, ServerConfig};

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
    pub auth_callback: Option<fn(track_name: String, auth_payload: String, AuthCallbackType)>,
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
    auth_callback: Option<fn(track_name: String, auth_payload: String, AuthCallbackType)>,
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

        if let UnderlayType::WebTransport = self.underlay {
        } else {
            bail!("Underlay must be WebTransport, not {:?}", self.underlay)
        }

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
            let incoming_session = server.accept().await;
            tokio::spawn(
                handle_connection(incoming_session)
                    .instrument(tracing::info_span!("Connection", id)),
            );
        }

        Ok(())
    }
}

async fn handle_connection(incoming_session: IncomingSession) {
    let result = handle_connection_impl(incoming_session).await;
    tracing::error!("{:?}", result);
}

async fn handle_connection_impl(incoming_session: IncomingSession) -> Result<()> {
    let mut buffer = vec![0; 65536].into_boxed_slice();

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

    tracing::info!("Waiting for data from client...");

    loop {
        tokio::select! {
            stream = connection.accept_bi() => {
                let span = tracing::info_span!("sid", stable_id);

                let mut stream = stream?;
                tracing::info!("Accepted BI stream");

                let bytes_read = match stream.1.read(&mut buffer).instrument(span).await? {
                    Some(bytes_read) => bytes_read,
                    None => continue,
                };

                let span = tracing::info_span!("sid", stable_id);
                let _ = span.in_scope(|| -> Result<()> {
                    let str_data = std::str::from_utf8(&buffer[..bytes_read])?;

                    tracing::info!("Received (bi) '{str_data}' from client");

                    Ok(())
                });

                stream.0.write_all(b"ACK").await?;
            },
            stream = connection.accept_uni() => {
                let span = tracing::info_span!("sid", stable_id);

                let mut stream = stream?;
                tracing::info!("Accepted UNI stream");

                let bytes_read = match stream.read(&mut buffer).instrument(span).await? {
                    Some(bytes_read) => bytes_read,
                    None => continue,
                };

                let span = tracing::info_span!("sid", stable_id);
                let _ = span.in_scope(|| -> Result<()> {
                    let str_data = std::str::from_utf8(&buffer[..bytes_read])?;

                    tracing::info!("Received (uni) '{str_data}' from client");

                    Ok(())
                });

                let mut stream = connection.open_uni().await?.await?;
                stream.write_all(b"ACK").await?;
            },
            // MOQTではdatagramは使わないため
            // dgram = connection.receive_datagram() => {
            //     let span = tracing::info_span!("sid", stable_id);

            //     let dgram = dgram?;
            //     let str_data = std::str::from_utf8(&dgram)?;

            //     let _ = span.in_scope(|| -> Result<()> {
            //         tracing::info!("Received (dgram) '{str_data}' from client");

            //         Ok(())
            //     });

            //     connection.send_datagram(b"ACK")?;
            // },
            _ = connection.closed() => {
                tracing::info!("Connection closed, rtt={:?}", connection.rtt());
                break;
            }
        }
    }

    Ok(())
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

// pub fn add(left: usize, right: usize) -> usize {
//     left + right
// }

// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[test]
//     fn it_works() {
//         let result = add(2, 2);
//         assert_eq!(result, 4);
//     }
// }
