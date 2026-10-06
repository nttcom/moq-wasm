use anyhow::{Context, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayEndpoint {
    pub relay_id: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClickHouseConfig {
    pub url: String,
    pub database: String,
    pub user: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservabilityConfig {
    pub relays: Vec<RelayEndpoint>,
    pub auth_token: Option<String>,
    pub verify_relay_certificate: bool,
    pub clickhouse: ClickHouseConfig,
    pub http_port: u16,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

pub fn parse_relays(value: &str) -> anyhow::Result<Vec<RelayEndpoint>> {
    let relays = value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (relay_id, url) = entry
                .split_once('=')
                .with_context(|| format!("relay entry {entry:?} is not RELAY_ID=URL"))?;
            Ok(RelayEndpoint {
                relay_id: relay_id.trim().to_string(),
                url: url.trim().to_string(),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    if relays.is_empty() {
        bail!("no relay configured");
    }
    Ok(relays)
}

impl ObservabilityConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let relays = parse_relays(
            &env("OBSERVABILITY_RELAYS")
                .context("OBSERVABILITY_RELAYS is required, e.g. relay-a=moqt://relay-a:4433")?,
        )?;
        let http_port = env("OBSERVABILITY_HTTP_PORT")
            .map(|port| port.parse::<u16>())
            .transpose()
            .context("OBSERVABILITY_HTTP_PORT must be a port number")?
            .unwrap_or(8095);
        Ok(Self {
            relays,
            auth_token: env("OBSERVABILITY_AUTH_TOKEN"),
            verify_relay_certificate: env("OBSERVABILITY_INSECURE").is_none(),
            clickhouse: ClickHouseConfig {
                url: env("CLICKHOUSE_URL").unwrap_or_else(|| "http://127.0.0.1:8123".to_string()),
                database: env("CLICKHOUSE_DATABASE").unwrap_or_else(|| "observability".to_string()),
                user: env("CLICKHOUSE_USER"),
                password: env("CLICKHOUSE_PASSWORD"),
            },
            http_port,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{RelayEndpoint, parse_relays};

    #[test]
    fn relays_are_comma_separated_id_url_pairs() {
        // Act
        let relays = parse_relays("relay-a=moqt://a:4433, relay-b = moqt://b:4433").unwrap();

        // Assert
        assert_eq!(
            relays,
            vec![
                RelayEndpoint {
                    relay_id: "relay-a".to_string(),
                    url: "moqt://a:4433".to_string(),
                },
                RelayEndpoint {
                    relay_id: "relay-b".to_string(),
                    url: "moqt://b:4433".to_string(),
                },
            ]
        );
    }

    #[test]
    fn an_entry_without_an_id_is_rejected() {
        // Act / Assert
        assert!(parse_relays("moqt://a:4433").is_err());
    }

    #[test]
    fn an_empty_list_is_rejected() {
        // Act / Assert
        assert!(parse_relays(" , ").is_err());
    }
}
