mod catalog;
mod cli;
mod loc;
mod media;
mod publish;
mod relay_url;
mod subscribe;
mod track;
mod transport;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, Command};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(std::io::stderr)
        .init();

    run(Cli::parse()).await
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Publish(args) => publish::run(args).await,
        Command::Subscribe(args) => subscribe::run(args).await,
    }
}
