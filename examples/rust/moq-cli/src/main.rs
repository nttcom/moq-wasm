mod catalog;
mod cli;
mod media;
mod publish;
mod subscribe;
mod track;
mod transport;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, Command};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(std::io::stderr)
        .init();

    if let Err(error) = run(Cli::parse()).await {
        eprintln!("Error: {error:#}");
        // A pending blocking read on stdin would otherwise keep the runtime
        // from shutting down until stdin delivers data or EOF.
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Publish(args) => publish::run(args).await,
        Command::Subscribe(args) => subscribe::run(args).await,
    }
}
