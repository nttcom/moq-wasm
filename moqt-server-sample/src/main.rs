use anyhow::{Ok, Result};
use clap::Parser;
use moqt_server::{MOQTConfig, MOQTServer, constants::UnderlayType};

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
    let current_path = std::env::current_dir().expect("failed to get current path");
    config.key_path = format!("{}{}", current_path.to_str().unwrap(), "/keys/key.pem");
    config.cert_path = format!("{}{}", current_path.to_str().unwrap(), "/keys/cert.pem");
    config.log_level = args.log_level.to_string();

    let moqt_server = MOQTServer::new(config);
    moqt_server.start().await?;

    Ok(())
}
