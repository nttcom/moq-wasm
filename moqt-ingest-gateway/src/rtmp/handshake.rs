use anyhow::{anyhow, Result};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

pub async fn perform_handshake(socket: &mut TcpStream, label: &str) -> Result<Vec<u8>> {
    let mut handshake = Handshake::new(PeerType::Server);
    let leftover;
    let mut buf = [0u8; 4096];

    loop {
        let read = socket.read(&mut buf).await?;
        if read == 0 {
            return Err(anyhow!("connection closed during handshake"));
        }

        match handshake.process_bytes(&buf[..read])? {
            HandshakeProcessResult::InProgress { response_bytes } => {
                socket.write_all(&response_bytes).await?;
            }
            HandshakeProcessResult::Completed {
                response_bytes,
                remaining_bytes,
            } => {
                socket.write_all(&response_bytes).await?;
                leftover = remaining_bytes;
                break;
            }
        }
    }

    println!("[rtmp {label}] handshake completed");
    Ok(leftover)
}
