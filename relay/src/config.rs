#[derive(Clone, Debug)]
pub struct RelayConfig {
    pub relay_id: String,
    pub advertise_host: String,
    pub port: u16,
    pub inner_port: u16,
    pub redis_url: Option<String>,
}

impl RelayConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let relay_id = std::env::var("RELAY_ID").unwrap_or_else(|_| "relay-local".to_string());
        let advertise_host =
            std::env::var("RELAY_ADVERTISE_HOST").unwrap_or_else(|_| "localhost".to_string());
        let port = std::env::var("RELAY_PORT")
            .ok()
            .map(|value| value.parse::<u16>())
            .transpose()?
            .unwrap_or(4433);
        let inner_port = std::env::var("RELAY_INNER_PORT")
            .ok()
            .map(|value| value.parse::<u16>())
            .transpose()?
            .unwrap_or(port + 1);
        let redis_url = std::env::var("REDIS_URL").ok();

        Ok(Self {
            relay_id,
            advertise_host,
            port,
            inner_port,
            redis_url,
        })
    }
}
