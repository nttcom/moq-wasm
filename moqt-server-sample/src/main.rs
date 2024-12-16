use anyhow::{Ok, Result};
use clap::Parser;
use moqt_server::{constants::UnderlayType, MOQTConfig, MOQTServer};

#[derive(Debug, Parser)]
struct Arg {
    #[arg(short = 'l', long = "log", help = "Log Level", default_value = "DEBUG")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Arg::parse();

    let mut config = MOQTConfig::new();
    config.underlay = UnderlayType::WebTransport;
    config.key_path = "./moqt-server-sample/keys/key.pem".to_string();
    config.cert_path = "./moqt-server-sample/keys/cert.pem".to_string();
    config.log_level = args.log_level.to_string();

    let moqt_server = MOQTServer::new(config);
    moqt_server.start().await?;

    Ok(())
}
