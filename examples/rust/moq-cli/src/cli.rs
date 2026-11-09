use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use url::Url;

use crate::track::{FullTrackName, Namespace};

#[derive(Parser, Debug)]
#[command(about = "MoQT CLI: publish/subscribe media over MoQT")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
    #[command(flatten)]
    pub telemetry: TelemetryArgs,
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Publish(_) => "publish",
            Self::Subscribe(_) => "subscribe",
        }
    }

    pub fn relay(&self) -> &RelayArgs {
        match self {
            Self::Publish(args) => &args.relay,
            Self::Subscribe(args) => &args.relay,
        }
    }
}

const LOCAL_TELEMETRY_URL: &str = "http://localhost:8095";
const CLOUD_TELEMETRY_URL: &str = "https://observability.moqt.research.skyway.io";
const LOCAL_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

#[derive(Args, Debug)]
pub struct TelemetryArgs {
    /// Observability server that receives the CLI's logs. Defaults to the
    /// local one for a local relay and the deployed one otherwise
    #[arg(long, env = "MOQT_TELEMETRY_URL", global = true)]
    pub telemetry: Option<Url>,
    /// Do not send the CLI's logs to an observability server
    #[arg(long, global = true, conflicts_with = "telemetry")]
    pub no_telemetry: bool,
}

impl TelemetryArgs {
    pub fn endpoint(&self, relay: &Url) -> Option<Url> {
        if self.no_telemetry {
            return None;
        }
        let base = match &self.telemetry {
            Some(url) => url.as_str().trim_end_matches('/').to_string(),
            None if LOCAL_HOSTS.contains(&relay.host_str().unwrap_or_default()) => {
                LOCAL_TELEMETRY_URL.to_string()
            }
            None => CLOUD_TELEMETRY_URL.to_string(),
        };
        Url::parse(&format!("{base}/api/client-telemetry")).ok()
    }
}

#[derive(Subcommand, Debug)]
pub enum Command {
    Publish(PublishArgs),
    Subscribe(SubscribeArgs),
}

#[derive(Args, Clone, Debug)]
pub struct RelayArgs {
    #[arg(long = "relay")]
    pub url: Url,
    #[arg(long)]
    pub insecure: bool,
    /// Authorization token (JWT) presented to the relay in CLIENT_SETUP
    #[arg(long, env = "MOQT_AUTH_TOKEN", conflicts_with = "auth_token_file")]
    pub auth_token: Option<String>,
    /// File holding the authorization token; re-read while connected so a
    /// rewritten token is sent to the relay before the current one expires
    #[arg(long, env = "MOQT_AUTH_TOKEN_FILE")]
    pub auth_token_file: Option<PathBuf>,
    /// MoQT drafts offered to the relay, comma-separated and most preferred
    /// first, each draft-14 or draft-18; draft-14 only when omitted
    #[arg(long, env = "MOQT_VERSIONS", value_delimiter = ',')]
    pub versions: Vec<moqt::Version>,
}

impl RelayArgs {
    pub fn offered_versions(&self) -> Vec<moqt::Version> {
        if self.versions.is_empty() {
            moqt::DEFAULT_VERSIONS.to_vec()
        } else {
            self.versions.clone()
        }
    }
}

#[derive(Args, Debug)]
pub struct PublishArgs {
    #[command(flatten)]
    pub relay: RelayArgs,
    /// Namespace of the broadcast, e.g. `anon/tokyo/cam01`
    #[arg(long)]
    pub namespace: Namespace,
}

#[derive(Args, Debug)]
pub struct SubscribeArgs {
    #[command(flatten)]
    pub relay: RelayArgs,
    #[arg(long)]
    pub track: FullTrackName,
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use url::Url;

    use super::{Cli, Command, TelemetryArgs};

    fn offered_versions(args: &[&str]) -> Vec<moqt::Version> {
        let cli = Cli::try_parse_from(
            [
                "moq-cli",
                "subscribe",
                "--relay",
                "moqt://localhost:4433",
                "--track",
                "a/b",
            ]
            .into_iter()
            .chain(args.iter().copied()),
        )
        .unwrap();
        let Command::Subscribe(subscribe) = cli.command else {
            panic!("expected the subscribe command");
        };
        subscribe.relay.offered_versions()
    }

    #[test]
    fn the_relay_is_offered_the_versions_given_in_their_order() {
        // Act / Assert
        assert_eq!(
            offered_versions(&["--versions", "draft-18,draft-14"]),
            [moqt::Version::Draft18, moqt::Version::Draft14]
        );
    }

    #[test]
    fn without_versions_the_default_is_offered() {
        // Act / Assert
        assert_eq!(offered_versions(&[]), moqt::DEFAULT_VERSIONS);
    }

    fn endpoint(telemetry: Option<&str>, no_telemetry: bool, relay: &str) -> Option<String> {
        let args = TelemetryArgs {
            telemetry: telemetry.map(|url| Url::parse(url).unwrap()),
            no_telemetry,
        };
        args.endpoint(&Url::parse(relay).unwrap())
            .map(|url| url.to_string())
    }

    #[test]
    fn a_local_relay_reports_to_the_local_server() {
        // Act / Assert
        assert_eq!(
            endpoint(None, false, "moqt://127.0.0.1:4433").as_deref(),
            Some("http://localhost:8095/api/client-telemetry")
        );
    }

    #[test]
    fn a_cloud_relay_reports_to_the_deployed_server() {
        // Act / Assert
        assert_eq!(
            endpoint(None, false, "https://relay-1.moqt.research.skyway.io:443").as_deref(),
            Some("https://observability.moqt.research.skyway.io/api/client-telemetry")
        );
    }

    #[test]
    fn a_configured_server_overrides_the_default() {
        // Act / Assert
        assert_eq!(
            endpoint(
                Some("http://obs.example:8095/"),
                false,
                "moqt://localhost:4433"
            )
            .as_deref(),
            Some("http://obs.example:8095/api/client-telemetry")
        );
    }

    #[test]
    fn no_telemetry_sends_nothing() {
        // Act / Assert
        assert_eq!(endpoint(None, true, "moqt://localhost:4433"), None);
    }
}
