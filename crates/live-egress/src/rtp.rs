use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
};

use anyhow::{Context, Result};
use mediapack::rtp::{Destination, Muxer, MuxerOptions};
use tokio::net::UdpSocket;

use crate::broadcast_reader::BroadcastReader;

pub struct RtpOutput {
    pub destination: Destination,
    pub muxer: MuxerOptions,
    /// Written once the muxer knows every stream, for the receiver to open.
    pub sdp_path: PathBuf,
}

/// Sends the broadcast's media as RTP until every track has ended. The
/// socket stays unconnected: a connected UDP socket reports the ICMP port
/// unreachable of a receiver that has not started yet as an error on the
/// next send, and RTP output must not depend on the receiver being up.
pub async fn run_rtp_output(reader: &mut BroadcastReader, output: &RtpOutput) -> Result<()> {
    let unspecified = match output.destination.address {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    };
    let socket = UdpSocket::bind(SocketAddr::new(unspecified, 0))
        .await
        .context("bind the RTP output socket")?;
    let mut muxer = Muxer::new(output.muxer);
    let mut sdp_written = false;
    while let Some(event) = reader.next_event().await {
        for packet in muxer.push(&event)? {
            let target = output.destination.rtp(packet.kind);
            socket
                .send_to(&packet.data, target)
                .await
                .with_context(|| format!("send RTP to {target}"))?;
        }
        if !sdp_written && let Some(description) = muxer.session_description(&output.destination) {
            tokio::fs::write(&output.sdp_path, description.to_string())
                .await
                .with_context(|| format!("write SDP {}", output.sdp_path.display()))?;
            tracing::info!(path = %output.sdp_path.display(), "SDP written; the RTP output is playable");
            sdp_written = true;
        }
    }
    Ok(())
}
