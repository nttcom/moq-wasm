mod broadcast_reader;
mod relay_session;
mod rtp;

use std::{net::IpAddr, path::PathBuf};

use anyhow::{Result, bail};
use clap::Parser;
use mediapack::rtp::{
    Destination, MuxerOptions,
    muxer::{AUDIO_PAYLOAD_TYPE, VIDEO_PAYLOAD_TYPE},
};
use moqt::ClientConfig;
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

use crate::{broadcast_reader::BroadcastReader, relay_session::RelaySession, rtp::RtpOutput};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Subscribe to an MSF broadcast and send it as plain RTP"
)]
struct Args {
    /// MoQ server URL (`moqt://` for QUIC, `https://` for WebTransport)
    #[arg(long)]
    moqt_url: String,

    /// Skip TLS certificate verification (INSECURE, local development only)
    #[arg(long)]
    insecure_skip_tls_verify: bool,

    /// Authorization token (JWT) presented to the relay in CLIENT_SETUP
    #[arg(long, env = "MOQT_AUTH_TOKEN")]
    auth_token: Option<String>,

    /// Namespace of the broadcast, e.g. anon/live/test
    #[arg(long)]
    namespace: String,

    /// Address the RTP packets are sent to
    #[arg(long, default_value = "127.0.0.1")]
    destination: IpAddr,

    /// UDP port of the H.264 stream at the destination
    #[arg(long, default_value_t = 5004)]
    video_port: u16,

    /// UDP port of the audio stream at the destination
    #[arg(long, default_value_t = 5006)]
    audio_port: u16,

    /// RTP payload type of the video stream; dynamic types only
    #[arg(long, default_value_t = VIDEO_PAYLOAD_TYPE, value_parser = clap::value_parser!(u8).range(96..=127))]
    video_payload_type: u8,

    /// RTP payload type of the audio stream; dynamic types only
    #[arg(long, default_value_t = AUDIO_PAYLOAD_TYPE, value_parser = clap::value_parser!(u8).range(96..=127))]
    audio_payload_type: u8,

    /// SSRC of the video stream; random when absent
    #[arg(long)]
    video_ssrc: Option<u32>,

    /// SSRC of the audio stream; random when absent
    #[arg(long)]
    audio_ssrc: Option<u32>,

    /// Send RTCP on each stream's RTP port instead of the port above it
    #[arg(long)]
    rtcp_mux: bool,

    /// Where the SDP describing the output is written for the receiver
    #[arg(long, default_value = "egress.sdp")]
    sdp_out: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .init();

    let namespace = normalize_namespace(&args.namespace)?;
    let output = args.rtp_output();
    let relay = RelaySession::connect(
        &args.moqt_url,
        &ClientConfig {
            port: 0,
            verify_certificate: !args.insecure_skip_tls_verify,
            authorization_token: args.auth_token,
            follow_go_away: true,
        },
    )
    .await?;
    let result = run(&relay, &namespace, &output).await;
    relay.close().await;
    result
}

impl Args {
    fn rtp_output(&self) -> RtpOutput {
        RtpOutput {
            destination: Destination {
                address: self.destination,
                video_port: self.video_port,
                audio_port: self.audio_port,
                rtcp_mux: self.rtcp_mux,
            },
            muxer: MuxerOptions {
                video_payload_type: self.video_payload_type,
                audio_payload_type: self.audio_payload_type,
                video_ssrc: self.video_ssrc,
                audio_ssrc: self.audio_ssrc,
            },
            sdp_path: self.sdp_out.clone(),
        }
    }
}

/// Drops empty elements the way `live-ingest` does, so `anon/live/test/`
/// names the namespace it published rather than one with a trailing empty
/// element.
fn normalize_namespace(text: &str) -> Result<String> {
    let parts: Vec<&str> = text.split('/').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        bail!("namespace {text:?} is empty");
    }
    Ok(parts.join("/"))
}

async fn run(relay: &RelaySession, namespace: &str, output: &RtpOutput) -> Result<()> {
    let mut reader = BroadcastReader::run(&relay.session, namespace).await?;
    tokio::select! {
        result = rtp::run_rtp_output(&mut reader, output) => result,
        close = relay.session.closed() => bail!("relay session ended: {close}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_empty_namespace_elements() {
        // Act / Assert
        assert_eq!(
            normalize_namespace("/anon/live//test/").unwrap(),
            "anon/live/test"
        );
        assert!(normalize_namespace("/").is_err());
    }

    #[test]
    fn takes_the_stream_identities_and_rtcp_port_from_the_command_line() {
        // Arrange
        let args = Args::parse_from([
            "live-egress",
            "--moqt-url",
            "moqt://127.0.0.1:4433",
            "--namespace",
            "anon/live/test",
            "--video-payload-type",
            "102",
            "--audio-ssrc",
            "4242",
            "--rtcp-mux",
        ]);

        // Act
        let output = args.rtp_output();

        // Assert
        assert_eq!(
            output.muxer,
            MuxerOptions {
                video_payload_type: 102,
                audio_payload_type: 97,
                video_ssrc: None,
                audio_ssrc: Some(4_242),
            }
        );
        assert!(output.destination.rtcp_mux);
    }

    #[test]
    fn rejects_static_payload_types() {
        // Act
        let parsed = Args::try_parse_from([
            "live-egress",
            "--moqt-url",
            "moqt://127.0.0.1:4433",
            "--namespace",
            "anon/live/test",
            "--audio-payload-type",
            "8",
        ]);

        // Assert
        assert!(parsed.is_err());
    }
}
