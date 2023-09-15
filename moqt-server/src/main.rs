use anyhow::Result;
use moqt::{constants::UnderlayType, MOQTConfig, MOQT};

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(feature = "foo")]
    {
        println!("{}", exec_cb(add));
    }
    #[cfg(not(feature = "foo"))]
    {
        let mut config = MOQTConfig::new();
        config.underlay = UnderlayType::WebTransport;
        config.key_path = "./moqt/keys/key.pem".to_string();
        config.cert_path = "./moqt/keys/cert.pem".to_string();

        let moqt = MOQT::new(config);
        moqt.start().await?;
    }
    Ok(())
}

fn exec_cb<T>(f: fn(u128, u128) -> T) -> T {
    f(1, 2)
}

fn add(a: u128, b: u128) -> u128 {
    a + b
}
