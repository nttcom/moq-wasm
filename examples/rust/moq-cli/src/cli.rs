use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use url::Url;

use crate::track::{FullTrackName, Namespace};

#[derive(Parser, Debug)]
#[command(about = "MoQT CLI: publish/subscribe media over MoQT")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
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
