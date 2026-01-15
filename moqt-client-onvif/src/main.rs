mod cli;
mod config;
mod http_client;
mod soap;
mod ptz;
mod ptz_control;
mod ptz_defs;
mod ptz_input;
mod ptz_parse;
mod ptz_service;
mod wsse;

use anyhow::Result;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    let args = cli::Args::parse();
    let target = config::Target::from_args(&args)?;
    ptz::interactive_control(&target).await
}
