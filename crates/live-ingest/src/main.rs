mod ingest;
mod ingest_publisher;
mod renditions;
mod rtmp;
mod srt;
mod stats_panel;

use anyhow::Result;
use clap::Parser;
use publisher::MoqtTarget;
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

use crate::{
    ingest_publisher::IngestOptions,
    stats_panel::{ConnectionRegistry, StatsPanel},
};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Listen RTMP/SRT and spawn handlers per stream"
)]
struct Args {
    /// RTMP listen address (e.g. 0.0.0.0:1935)
    #[arg(long, default_value = "0.0.0.0:1935")]
    rtmp_addr: String,

    /// SRT listen address (e.g. 0.0.0.0:9000)
    #[arg(long, default_value = "0.0.0.0:9000")]
    srt_addr: String,

    /// MoQ server URL (`moqt://` for QUIC, `https://` for WebTransport)
    #[arg(long)]
    moqt_url: Option<String>,

    /// Skip TLS certificate verification (INSECURE, local development only)
    #[arg(long)]
    insecure_skip_tls_verify: bool,

    /// Authorization token (JWT) presented to the relay in CLIENT_SETUP
    #[arg(long, env = "MOQT_AUTH_TOKEN")]
    auth_token: Option<String>,

    /// Re-encode video into the standard renditions below the source resolution
    #[arg(long)]
    transcode: bool,

    /// Redraw the QUIC statistics of every relay connection on stdout once a second; logs move to stderr
    #[arg(long)]
    stats: bool,
}

impl Args {
    fn moqt_target(&self) -> Option<MoqtTarget> {
        self.moqt_url.clone().map(|url| MoqtTarget {
            url,
            auth_token: self.auth_token.clone(),
            verify_certificate: !self.insecure_skip_tls_verify,
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let log_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    let logs = tracing_subscriber::fmt().with_env_filter(log_filter);
    let stats = if args.stats {
        logs.with_writer(std::io::stderr).init();
        let registry = ConnectionRegistry::default();
        Some((StatsPanel::run(registry.clone()), registry))
    } else {
        logs.init();
        None
    };

    let options = IngestOptions {
        moqt: args.moqt_target(),
        transcode: args.transcode,
        stats: stats.as_ref().map(|(_, registry)| registry.clone()),
    };
    tokio::try_join!(
        rtmp::run_rtmp_listener(args.rtmp_addr, options.clone()),
        srt::run_srt_listener(args.srt_addr, options),
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELAY_URL: &str = "https://relay.example.com:443";

    #[test]
    fn moqt_target_verifies_the_relay_certificate_by_default() {
        // Arrange
        let args = Args::parse_from(["live-ingest", "--moqt-url", RELAY_URL]);

        // Act
        let target = args.moqt_target().unwrap();

        // Assert
        assert!(target.verify_certificate);
    }

    #[test]
    fn moqt_target_skips_certificate_verification_when_requested() {
        // Arrange
        let args = Args::parse_from([
            "live-ingest",
            "--moqt-url",
            RELAY_URL,
            "--insecure-skip-tls-verify",
        ]);

        // Act
        let target = args.moqt_target().unwrap();

        // Assert
        assert!(!target.verify_certificate);
    }
}
