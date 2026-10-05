use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use clap::Parser;
use vts::{MintRequest, mint_token, parse_apps};

/// Signs a token for an app registered in the apps file
#[derive(Parser)]
struct Args {
    /// Registered apps (JSON array, see apps.example.json)
    #[arg(long)]
    apps: PathBuf,

    #[command(flatten)]
    request: MintRequest,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let text = fs::read_to_string(&args.apps)
        .with_context(|| format!("failed to read apps file {}", args.apps.display()))?;
    let apps = parse_apps(&text)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let token = mint_token(&apps, &args.request, now)
        .with_context(|| format!("in {}", args.apps.display()))?;
    println!("{token}");
    Ok(())
}
