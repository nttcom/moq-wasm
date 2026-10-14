use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use mediapack::{
    Timestamp,
    rtp::{Destination, MediaKind, Muxer, MuxerOptions},
};
use tokio::{net::UdpSocket, time::Instant};

use crate::broadcast_reader::BroadcastReader;

/// The minimum RTCP interval RFC 3550 §6.2 recommends. A stream's first
/// report goes out with its first packet instead, so that a receiver aligns
/// the streams from the start.
const SENDER_REPORT_INTERVAL: Duration = Duration::from_secs(5);

pub struct RtpOutput {
    pub destination: Destination,
    pub muxer: MuxerOptions,
    /// Written once the muxer knows every stream, for the receiver to open.
    pub sdp_path: PathBuf,
}

/// Sends the broadcast's media as RTP, with RTCP sender reports, until every
/// track has ended. The socket stays unconnected: a connected UDP socket
/// reports the ICMP port unreachable of a receiver that has not started yet
/// as an error on the next send, and RTP output must not depend on the
/// receiver being up.
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
    let mut next_report = Instant::now();
    let mut reported: Vec<MediaKind> = Vec::new();
    while let Some(event) = reader.next_event().await {
        let packets = muxer.push(&event)?;
        let new_stream = packets
            .iter()
            .any(|packet| !reported.contains(&packet.kind));
        for packet in &packets {
            let target = output.destination.rtp(packet.kind);
            socket
                .send_to(&packet.data, target)
                .await
                .with_context(|| format!("send RTP to {target}"))?;
        }
        if (new_stream || Instant::now() >= next_report)
            && let Some(capture_origin) = reader.capture_origin()
        {
            for report in muxer.sender_reports(capture_origin, wall_clock()) {
                let target = output.destination.rtcp(report.kind);
                socket
                    .send_to(&report.data, target)
                    .await
                    .with_context(|| format!("send RTCP to {target}"))?;
                if !reported.contains(&report.kind) {
                    reported.push(report.kind);
                }
            }
            next_report = Instant::now() + SENDER_REPORT_INTERVAL;
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

fn wall_clock() -> Timestamp {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    Timestamp::from_micros(since_epoch.as_micros() as u64)
}
