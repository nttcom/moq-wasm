use bytes::Bytes;
use tokio::{net::UdpSocket, sync::mpsc};

/// The largest UDP payload; RTP senders stay far below it.
const MAX_DATAGRAM: usize = 65_535;

/// Returns when the socket fails or the channel is closed.
pub(super) async fn receive_datagrams(socket: UdpSocket, packet_sender: mpsc::Sender<Bytes>) {
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
}
