mod moqt_receiver;

use anyhow::{Context as _, Result};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: wt-subscriber <namespace> <track_name>");
        eprintln!("Example: cargo run -p wt-subscriber -- live-1123 video | ffplay -");
        std::process::exit(1);
    }
    let namespace = &args[1];
    let track_name = &args[2];

    moqt_receiver::subscribe_and_receive(namespace, track_name)
        .await
        .context("subscriber failed")?;

    Ok(())
}
