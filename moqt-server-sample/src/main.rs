use anyhow::{Ok, Result};
use moqt_server::{constants::UnderlayType, AuthCallbackType, MOQTConfig, MOQT};

#[tokio::main]
async fn main() -> Result<()> {
    let mut config = MOQTConfig::new();
    config.underlay = UnderlayType::WebTransport;
    config.key_path = "./moqt-server-sample/keys/key.pem".to_string();
    config.cert_path = "./moqt-server-sample/keys/cert.pem".to_string();

    // config.auth_callback = Some(|track_name: String, auth_payload: String, auth_callback_type: AuthCallbackType| {Ok(())});

    let moqt = MOQT::new(config);
    moqt.start().await?;

    Ok(())
}
