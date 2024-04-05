use anyhow::{Ok, Result};
use clap::Parser;
use moqt_server::{constants::UnderlayType, MOQTConfig, MOQT};

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

    // config.auth_callback = Some(|track_name: String, auth_payload: String, auth_callback_type: AuthCallbackType| {Ok(())});

    let moqt = MOQT::new(config);
    moqt.start().await?;

    Ok(())
}
