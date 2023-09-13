use std::time::Duration;

use anyhow::Result;
use tracing::{self, Instrument};
use tracing_subscriber::{self, filter::LevelFilter, EnvFilter};
use wtransport::{endpoint::IncomingSession, tls::Certificate, Endpoint, ServerConfig};

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();

    let config = ServerConfig::builder()
        .with_bind_default(4433)
        .with_certificate(Certificate::load("./keys/cert.pem", "./keys/key.pem")?)
        .keep_alive_interval(Some(Duration::from_secs(3)))
        .build();

    let server = Endpoint::server(config)?;

    tracing::info!("Server ready!");

    for id in 0.. {
        let incoming_session = server.accept().await;
        tokio::spawn(
            handle_connection(incoming_session).instrument(tracing::info_span!("Connection", id)),
        );
    }

    Ok(())
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
                let mut stream = stream?;
                tracing::info!("Accepted BI stream");

                let bytes_read = match stream.1.read(&mut buffer).await? {
                    Some(bytes_read) => bytes_read,
                    None => continue,
                };

                let str_data = std::str::from_utf8(&buffer[..bytes_read])?;

                tracing::info!("Received (bi) '{str_data}' from client");

                stream.0.write_all(b"ACK").await?;
            },
            stream = connection.accept_uni() => {
                let mut stream = stream?;
                tracing::info!("Accepted UNI stream");

                let bytes_read = match stream.read(&mut buffer).await? {
                    Some(bytes_read) => bytes_read,
                    None => continue,
                };

                let str_data = std::str::from_utf8(&buffer[..bytes_read])?;

                tracing::info!("Received (uni) '{str_data}' from client");

                let mut stream = connection.open_uni().await?.await?;
                stream.write_all(b"ACK").await?;
            },
            dgram = connection.receive_datagram() => {
                let dgram = dgram?;
                let str_data = std::str::from_utf8(&dgram)?;

                tracing::info!("Received (dgram) '{str_data}' from client");

                connection.send_datagram(b"ACK")?;
            },
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
