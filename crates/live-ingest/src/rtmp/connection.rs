use std::collections::VecDeque;

use anyhow::{Context, Result};
use rml_rtmp::sessions::{ServerSession, ServerSessionConfig, ServerSessionResult};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use super::{
    handshake::perform_handshake,
    session::{RtmpState, handle_event},
};
use crate::ingest_publisher::IngestOptions;

pub async fn run_rtmp_listener(addr: String, options: IngestOptions) -> Result<()> {
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind RTMP listener on {addr}"))?;
    tracing::info!(%addr, "RTMP listener started");

    loop {
        let (socket, peer) = listener.accept().await?;
        let label = peer.to_string();
        let options = options.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(socket, &label, &options).await {
                tracing::warn!(peer = %label, ?err, "RTMP connection failed");
            }
        });
    }
}

async fn handle_connection(
    mut socket: TcpStream,
    label: &str,
    options: &IngestOptions,
) -> Result<()> {
    tracing::debug!(peer = %label, "RTMP handshake started");
    let mut buf = [0u8; 4096];
    let leftover = perform_handshake(&mut socket, label).await?;

    let (mut session, initial_results) =
        ServerSession::new(ServerSessionConfig::new()).context("create RTMP session")?;
    let mut state = RtmpState::new(options, label.to_string());

    handle_results(
        &mut session,
        &mut socket,
        initial_results,
        label,
        &mut state,
    )
    .await?;
    if !leftover.is_empty() {
        let results = session.handle_input(&leftover)?;
        handle_results(&mut session, &mut socket, results, label, &mut state).await?;
    }

    tracing::info!(peer = %label, "RTMP session ready");
    loop {
        let read = socket.read(&mut buf).await?;
        if read == 0 {
            tracing::info!(peer = %label, "RTMP client closed");
            break;
        }

        let results = session.handle_input(&buf[..read])?;
        handle_results(&mut session, &mut socket, results, label, &mut state).await?;
    }

    Ok(())
}

async fn handle_results(
    session: &mut ServerSession,
    socket: &mut TcpStream,
    results: Vec<ServerSessionResult>,
    label: &str,
    state: &mut RtmpState,
) -> Result<()> {
    let mut queue: VecDeque<ServerSessionResult> = results.into();

    while let Some(result) = queue.pop_front() {
        match result {
            ServerSessionResult::OutboundResponse(packet) => socket
                .write_all(&packet.bytes)
                .await
                .context("send RTMP packet")?,
            ServerSessionResult::RaisedEvent(event) => {
                handle_event(session, &mut queue, event, label, state).await?;
            }
            ServerSessionResult::UnhandleableMessageReceived(payload) => {
                tracing::warn!(peer = %label, type_id = payload.type_id, size = payload.data.len(), "unhandled RTMP message");
            }
        }
    }

    Ok(())
}
