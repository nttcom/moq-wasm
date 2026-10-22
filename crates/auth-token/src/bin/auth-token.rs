use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use auth_token::{Claims, parse_lifetime, sign_token};
use clap::Parser;

const SECRET_ENV: &str = "AUTH_TOKEN_SECRET";

/// Signs a client JWT with the app's secret, read from AUTH_TOKEN_SECRET
#[derive(Parser)]
struct Args {
    /// appId of the app whose secret signs the token
    #[arg(long)]
    app_id: String,

    /// Namespace path under the appId the token may publish to; "" grants the whole app
    #[arg(long)]
    publish: Option<String>,

    /// Namespace path under the appId the token may subscribe to; "" grants the whole app
    #[arg(long)]
    subscribe: Option<String>,

    /// Token lifetime, e.g. 30m, 12h, 365d
    #[arg(long, default_value = "12h", value_parser = parse_lifetime)]
    ttl: Duration,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let secret = std::env::var(SECRET_ENV)
        .ok()
        .filter(|secret| !secret.is_empty())
        .with_context(|| format!("set {SECRET_ENV} to the app's secret"))?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let claims = Claims::issued(args.app_id, args.publish, args.subscribe, now, args.ttl);
    println!("{}", sign_token(&claims, &secret)?);
    Ok(())
}
