mod listen;
mod pull;

use anyhow::Result;
use broadcast::MoqtTarget;
use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Bridge RTMP/SRT/RTP pushes and RTSP/ONVIF cameras over MoQ"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Listen for RTMP/SRT/RTP and publish every stream it receives
    Listen(listen::Args),
    /// Pull an RTSP stream or an ONVIF camera's profiles and publish them; with --onvif-ip, apply PTZ commands to the camera
    Pull(pull::Args),
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
        Command::Pull(args) => pull::run(args).await,
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
            Command::Pull(_) => unreachable!("parsed the listen subcommand"),
        }
    }

    #[test]
    fn listen_requires_an_input() {
        // Act
        let result = Cli::try_parse_from(["ingest-gateway", "listen", "--moqt-url", RELAY_URL]);

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn listen_accepts_a_single_input() {
        // Act
        let relay = listen_relay_args(&["--srt-addr", "0.0.0.0:9000"]);

        // Assert
        assert!(relay.moqt_target().is_none());
    }

    #[test]
    fn pull_requires_a_source() {
        // Act
        let result = Cli::try_parse_from(["ingest-gateway", "pull", "--moqt-url", RELAY_URL]);

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn pull_accepts_an_rtsp_url_without_a_camera() {
        // Act
        let result = Cli::try_parse_from([
            "ingest-gateway",
            "pull",
            "--rtsp-url",
            "rtsp://127.0.0.1:8554/test",
        ]);

        // Assert
        assert!(result.is_ok());
    }

    #[test]
    fn pull_requires_the_credentials_of_an_onvif_camera() {
        // Act
        let result = Cli::try_parse_from([
            "ingest-gateway",
            "pull",
            "--onvif-ip",
            "192.0.2.1",
            "--onvif-username",
            "admin",
        ]);

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn moqt_target_verifies_the_relay_certificate_by_default() {
        // Arrange
        let relay = listen_relay_args(&["--srt-addr", "0.0.0.0:9000", "--moqt-url", RELAY_URL]);

        // Act
        let target = relay.moqt_target().unwrap();

        // Assert
        assert!(target.verify_certificate);
    }

    #[test]
    fn moqt_target_skips_certificate_verification_when_requested() {
        // Arrange
        let relay = listen_relay_args(&[
            "--srt-addr",
            "0.0.0.0:9000",
            "--moqt-url",
            RELAY_URL,
            "--insecure-skip-tls-verify",
        ]);

        // Act
        let target = relay.moqt_target().unwrap();

        // Assert
        assert!(!target.verify_certificate);
    }
}
