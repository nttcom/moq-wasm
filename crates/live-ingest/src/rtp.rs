mod receive_task;

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use broadcast::{Broadcast, RelayConnection};
use bytes::Bytes;
use mediapack::rtp::{self, Codec, SessionDescription};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::{net::UdpSocket, sync::mpsc, time::timeout};

use crate::{
    ingest_publisher::{IngestOptions, IngestPublisher},
    namespace::split_namespace,
    rtp::receive_task::ReceiveTask,
};

/// A keyframe arrives as a burst of several hundred kilobytes within a few
/// milliseconds. The kernel default buffer (208 KiB on Linux) overflows
/// whenever the reader is not scheduled at once, and RTP has no retransmission.
const UDP_RECEIVE_BUFFER: usize = 4 * 1024 * 1024;
/// RTP carries no end-of-stream signal, so silence this long ends the broadcast.
const IDLE_TIMEOUT: Duration = Duration::from_secs(5);
const CHANNEL_CAPACITY: usize = 1_024;

/// Receives the RTP session described by `sdp_path` and publishes it under
/// `namespace`, one broadcast per run of datagrams. Fails when the SDP cannot
/// be read, lists no H.264 or AAC stream, or a port cannot be bound.
pub async fn run_rtp_listener(
    sdp_path: PathBuf,
    namespace: Vec<String>,
    options: IngestOptions,
) -> Result<()> {
    let text = tokio::fs::read_to_string(&sdp_path)
        .await
        .with_context(|| format!("read SDP {}", sdp_path.display()))?;
    let description = SessionDescription::parse(&text)
        .with_context(|| format!("parse SDP {}", sdp_path.display()))?;
    rtp::Demuxer::new(&description)?;
    let (packet_sender, mut packet_receiver) = mpsc::channel(CHANNEL_CAPACITY);
    let mut receive_tasks = Vec::new();
    for media in &description.media {
        if let Codec::Unsupported(encoding) = &media.codec {
            tracing::warn!(%encoding, port = media.port, "RTP stream is not bridged");
            continue;
        }
        let socket = bind(media.port, description.destination_of(media)).await?;
        tracing::info!(port = media.port, kind = ?media.kind, "RTP listener started");
        receive_tasks.push(ReceiveTask::run(socket, packet_sender.clone()));
    }
    drop(packet_sender);
    let label = namespace.join("/");
    while let Some(first) = packet_receiver.recv().await {
        tracing::info!(namespace = %label, "RTP stream started");
        match publish(
            first,
            &mut packet_receiver,
            &description,
            &options,
            namespace.clone(),
        )
        .await
        {
            Ok(summary) => tracing::info!(
                namespace = %label,
                packets = summary.packets,
                discontinuities = summary.discontinuities,
                "RTP stream ended"
            ),
            Err(err) => tracing::warn!(namespace = %label, ?err, "RTP stream failed"),
        }
    }
    Ok(())
}

pub fn parse_namespace(text: &str) -> Result<Vec<String>> {
    let parts = split_namespace(text);
    ensure!(!parts.is_empty(), "RTP namespace {text:?} is empty");
    Ok(parts)
}

struct StreamSummary {
    packets: u64,
    discontinuities: u64,
}

async fn publish(
    first: Bytes,
    packet_receiver: &mut mpsc::Receiver<Bytes>,
    description: &SessionDescription,
    options: &IngestOptions,
    namespace: Vec<String>,
) -> Result<StreamSummary> {
    let mut demuxer = rtp::Demuxer::new(description)?;
    let connection = RelayConnection::new(options.moqt.clone());
    let _registration = options
        .stats
        .as_ref()
        .map(|registry| registry.register(format!("rtp {}", namespace.join("/")), &connection));
    let mut publisher =
        IngestPublisher::new(Broadcast::new(connection, namespace), options.transcode);
    let mut packets = 0_u64;
    let mut discontinuities = 0_u64;
    let mut next = Some(first);
    while let Some(datagram) = next {
        packets += 1;
        for event in demuxer.push(&datagram)? {
            publisher.push(event)?;
        }
        if demuxer.discontinuities() > discontinuities {
            discontinuities = demuxer.discontinuities();
            tracing::warn!(
                discontinuities,
                "RTP packets lost; frames are dropped until the next keyframe"
            );
        }
        next = match timeout(IDLE_TIMEOUT, packet_receiver.recv()).await {
            Ok(Some(datagram)) => Some(datagram),
            Ok(None) | Err(_) => None,
        };
    }
    for event in demuxer.finish()? {
        publisher.push(event)?;
    }
    Ok(StreamSummary {
        packets,
        discontinuities,
    })
}

/// Binds the port on every interface and joins `destination` when it is a
/// multicast group.
async fn bind(port: u16, destination: Option<IpAddr>) -> Result<UdpSocket> {
    let (domain, unspecified) = match destination {
        Some(IpAddr::V6(_)) => (Domain::IPV6, IpAddr::V6(Ipv6Addr::UNSPECIFIED)),
        _ => (Domain::IPV4, IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
    };
    let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_recv_buffer_size(UDP_RECEIVE_BUFFER)?;
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket
        .bind(&SocketAddr::new(unspecified, port).into())
        .with_context(|| format!("bind UDP port {port}"))?;
    match destination {
        Some(IpAddr::V4(group)) if group.is_multicast() => {
            socket.join_multicast_v4(&group, &Ipv4Addr::UNSPECIFIED)?;
        }
        Some(IpAddr::V6(group)) if group.is_multicast() => {
            socket.join_multicast_v6(&group, 0)?;
        }
        _ => {}
    }
    UdpSocket::from_std(socket.into()).context("register the UDP socket with tokio")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_an_empty_namespace() {
        // Act / Assert
        assert!(parse_namespace("/").is_err());
    }

    #[tokio::test]
    async fn binds_an_ephemeral_port_with_a_large_receive_buffer() {
        // Act
        let socket = bind(0, Some(IpAddr::V4(Ipv4Addr::LOCALHOST)))
            .await
            .unwrap();

        // Assert
        assert_ne!(socket.local_addr().unwrap().port(), 0);
    }
}
