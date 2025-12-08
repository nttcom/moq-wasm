use std::collections::VecDeque;

use anyhow::{Context, Result};
use rml_rtmp::{
    chunk_io::Packet,
    sessions::{ServerSession, ServerSessionConfig, ServerSessionResult},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use super::{
    handshake::perform_handshake,
    session::{handle_event, RtmpState},
};
use crate::moqt::MoqtManager;

pub async fn run_rtmp_listener(addr: String, moqt_url: Option<String>) -> Result<()> {
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind RTMP listener on {addr}"))?;
    println!("RTMP listening on {addr}");

    loop {
        let (socket, peer) = listener.accept().await?;
        let label = peer.to_string();
        let moqt = MoqtManager::new(moqt_url.clone());
        tokio::spawn(async move {
            if let Err(err) = handle_connection(socket, &label, moqt).await {
                eprintln!("[rtmp {label}] error: {err:?}");
            }
        });
    }
}

async fn handle_connection(mut socket: TcpStream, label: &str, moqt: MoqtManager) -> Result<()> {
    println!("[rtmp {label}] handshake start");
    let mut buf = [0u8; 4096];
    let leftover = perform_handshake(&mut socket, label).await?;

    let (mut session, initial_results) =
        ServerSession::new(ServerSessionConfig::new()).context("create RTMP session")?;
    let mut state = RtmpState::new(moqt);

    handle_results(
        &mut session,
        &mut socket,
        initial_results,
        label,
        &mut state,
    )
    .await?;
    if !leftover.is_empty() {
        handle_session_bytes(&mut session, &mut socket, &leftover, label, &mut state).await?;
    }

    println!("[rtmp {label}] ready for streaming");
    loop {
        let read = socket.read(&mut buf).await?;
        if read == 0 {
            println!("[rtmp {label}] client closed");
            break;
        }

        handle_session_bytes(
            &mut session,
            &mut socket,
            &buf[..read],
            label,
            &mut state,
        )
        .await?;
    }

    Ok(())
}

async fn handle_session_bytes(
    session: &mut ServerSession,
    socket: &mut TcpStream,
    bytes: &[u8],
    label: &str,
    state: &mut RtmpState,
) -> Result<()> {
    let results = session.handle_input(bytes)?;
    handle_results(session, socket, results, label, state).await
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
            ServerSessionResult::OutboundResponse(packet) => send_packet(socket, &packet).await?,
            ServerSessionResult::RaisedEvent(event) => {
                handle_event(session, &mut queue, event, label, state).await?;
            }
            ServerSessionResult::UnhandleableMessageReceived(payload) => {
                eprintln!(
                    "[rtmp {label}] unhandled message type_id={} size={}",
                    payload.type_id,
                    payload.data.len()
                );
            }
        }
    }

    Ok(())
}

async fn send_packet(socket: &mut TcpStream, packet: &Packet) -> Result<()> {
    socket
        .write_all(&packet.bytes)
        .await
        .context("send RTMP packet")
}
