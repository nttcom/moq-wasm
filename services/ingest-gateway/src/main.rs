mod listen;
mod onvif;

use anyhow::Result;
use broadcast::MoqtTarget;
use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Bridge RTMP/SRT/RTP pushes and ONVIF cameras over MoQ"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Listen for RTMP/SRT/RTP and publish every stream it receives
    Listen(listen::Args),
    /// Publish an ONVIF camera's RTSP profiles and apply the PTZ commands it subscribes to
    Onvif(onvif::Args),
}

#[derive(Args, Debug)]
struct RelayArgs {
    /// MoQ relay URL (`moqt://` for QUIC, `https://` for WebTransport)
    #[arg(long)]
    moqt_url: Option<String>,

    /// Skip TLS certificate verification (INSECURE, local development only)
    #[arg(long)]
    insecure_skip_tls_verify: bool,

    /// Authorization token (JWT) presented to the relay in CLIENT_SETUP
    #[arg(long, env = "MOQT_AUTH_TOKEN")]
    auth_token: Option<String>,
}

impl RelayArgs {
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
    let cli = Cli::parse();
    let log_output = match &cli.command {
        Command::Listen(args) if args.stats => telemetry_otlp::LogOutput::Stderr,
        _ => telemetry_otlp::LogOutput::Stdout,
    };
    let _telemetry = telemetry_otlp::init(telemetry_otlp::TelemetryConfig {
        service_name: "ingest-gateway",
        service_version: env!("CARGO_PKG_VERSION"),
        default_directives: &[
            "ingest_gateway=info",
            "moqt=info",
            "broadcast=info",
            "mediapack=info",
            "transcode=info",
        ],
        log_output,
        ..Default::default()
    })?;
    match cli.command {
        Command::Listen(args) => listen::run(args).await,
        Command::Onvif(args) => onvif::run(args).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELAY_URL: &str = "https://relay.example.com:443";

    fn listen_relay_args(flags: &[&str]) -> RelayArgs {
        let cli = Cli::parse_from(["ingest-gateway", "listen"].iter().chain(flags));
        match cli.command {
            Command::Listen(args) => args.relay,
            Command::Onvif(_) => unreachable!("parsed the listen subcommand"),
        }
    }

    #[test]
    fn moqt_target_verifies_the_relay_certificate_by_default() {
        // Arrange
        let relay = listen_relay_args(&["--moqt-url", RELAY_URL]);

        // Act
        let target = relay.moqt_target().unwrap();

        // Assert
        assert!(target.verify_certificate);
    }

    #[test]
    fn moqt_target_skips_certificate_verification_when_requested() {
        // Arrange
        let relay = listen_relay_args(&["--moqt-url", RELAY_URL, "--insecure-skip-tls-verify"]);

        // Act
        let target = relay.moqt_target().unwrap();

        // Assert
        assert!(!target.verify_certificate);
    }
}
