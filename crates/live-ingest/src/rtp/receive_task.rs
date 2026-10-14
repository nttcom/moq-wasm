use bytes::Bytes;
use tokio::{net::UdpSocket, sync::mpsc, task::JoinHandle};

/// The largest UDP payload; RTP senders stay far below it.
const MAX_DATAGRAM: usize = 65_535;

/// Reads the datagrams of one socket into the session's packet channel
/// until the socket fails or the channel is closed.
pub(super) struct ReceiveTask {
    handle: JoinHandle<()>,
}

impl ReceiveTask {
    pub(super) fn run(socket: UdpSocket, packet_sender: mpsc::Sender<Bytes>) -> Self {
        let handle = tokio::spawn(async move {
            let mut buffer = vec![0_u8; MAX_DATAGRAM];
            loop {
                let length = match socket.recv(&mut buffer).await {
                    Ok(length) => length,
                    Err(err) => {
                        tracing::warn!(?err, "RTP socket read failed");
                        break;
                    }
                };
                let datagram = Bytes::copy_from_slice(&buffer[..length]);
                if packet_sender.send(datagram).await.is_err() {
                    break;
                }
            }
        });
        Self { handle }
    }
}

impl Drop for ReceiveTask {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
