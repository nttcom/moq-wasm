use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::relay_url::RelayUrl;
use crate::track::FullTrackName;

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

#[derive(ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[value(rename_all = "lower")]
pub enum Container {
    #[default]
    Loc,
    Cmaf,
}

#[derive(Args, Debug)]
pub struct PublishArgs {
    #[arg(long)]
    pub relay: RelayUrl,
    #[arg(long)]
    pub track: FullTrackName,
    #[arg(long)]
    pub insecure: bool,
    #[arg(long, default_value = "avc3")]
    pub codec: String,
    #[arg(long, value_enum, default_value_t = Container::Loc)]
    pub container: Container,
}

#[derive(Args, Debug)]
pub struct SubscribeArgs {
    #[arg(long)]
    pub relay: RelayUrl,
    #[arg(long)]
    pub track: FullTrackName,
    #[arg(long)]
    pub insecure: bool,
}
