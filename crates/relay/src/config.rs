use std::{path::PathBuf, time::Duration};

use anyhow::bail;

use crate::modules::auth::token_claims::ClaimPolicy;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthConfig {
    pub verify_url: String,
    pub relay_token: String,
    pub claim_policy: ClaimPolicy,
}

#[derive(Default)]
struct AuthEnv {
    vts_url: Option<String>,
    relay_token: Option<String>,
    max_token_ttl_seconds: Option<String>,
    clock_leeway_seconds: Option<String>,
}

fn non_negative_integer(value: Option<String>, name: &str, default: u64) -> anyhow::Result<u64> {
    match value {
        None => Ok(default),
        Some(text) => text
            .parse::<u64>()
            .map_err(|_| anyhow::anyhow!("{name} must be a non-negative integer, got {text:?}")),
    }
}

fn seconds(value: Option<String>, name: &str, default: Duration) -> anyhow::Result<Duration> {
    non_negative_integer(value, name, default.as_secs()).map(Duration::from_secs)
}

fn port_number(value: Option<String>, name: &str) -> anyhow::Result<Option<u16>> {
    value
        .map(|text| {
            text.parse::<u16>()
                .map_err(|_| anyhow::anyhow!("{name} must be a port number, got {text:?}"))
        })
        .transpose()
}

impl AuthConfig {
    fn from_env_values(env: AuthEnv) -> anyhow::Result<Self> {
        let (Some(verify_url), Some(relay_token)) = (env.vts_url, env.relay_token) else {
            bail!(
                "authentication is not configured: both AUTH_VTS_URL and AUTH_RELAY_TOKEN are required"
            );
        };
        let defaults = ClaimPolicy::default();
        let claim_policy = ClaimPolicy {
            max_token_ttl: seconds(
                env.max_token_ttl_seconds,
                "AUTH_MAX_TOKEN_TTL_SECONDS",
                defaults.max_token_ttl,
            )?,
            clock_leeway: seconds(
                env.clock_leeway_seconds,
                "AUTH_CLOCK_LEEWAY_SECONDS",
                defaults.clock_leeway,
            )?,
        };
        Ok(Self {
            verify_url,
            relay_token,
            claim_policy,
        })
    }
}

/// Disk tier for the payloads of objects received from client publishers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskCacheConfig {
    pub dir: PathBuf,
    /// Cap on the segments of all tracks together; the oldest segments go
    /// first once it is exceeded.
    pub max_bytes: u64,
}

const DEFAULT_DISK_CACHE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

impl DiskCacheConfig {
    /// `None` without a directory: the cache then stays in memory only.
    fn from_env_values(
        dir: Option<String>,
        max_bytes: Option<String>,
    ) -> anyhow::Result<Option<Self>> {
        let Some(dir) = dir else {
            return Ok(None);
        };
        let max_bytes = non_negative_integer(
            max_bytes,
            "RELAY_DISK_CACHE_BYTES",
            DEFAULT_DISK_CACHE_BYTES,
        )?;
        Ok(Some(Self {
            dir: PathBuf::from(dir),
            max_bytes,
        }))
    }
}

#[derive(Clone, Debug)]
pub struct RelayConfig {
    pub relay_id: String,
    pub advertise_host: String,
    /// `host[:port]` clients connect to; a draining relay publishes it so
    /// that other relays can name it in GOAWAY.
    pub public_host: String,
    pub port: u16,
    pub inner_port: u16,
    pub redis_url: Option<String>,
    /// How long a shutdown waits for the clients to leave after GOAWAY
    /// before closing them with GOAWAY_TIMEOUT.
    pub drain_timeout: Duration,
    pub auth: AuthConfig,
    pub disk_cache: Option<DiskCacheConfig>,
}

/// `KEY=` in an env file and compose `${KEY:-}` set the variable to an empty
/// string.
pub(crate) fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Shared by `init_logging`, which runs before `RelayConfig::from_env`, so logs
/// and config carry the same id.
pub(crate) fn relay_id() -> String {
    non_empty_env("RELAY_ID").unwrap_or_else(|| "relay-local".to_string())
}

impl RelayConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let relay_id = relay_id();
        let advertise_host =
            non_empty_env("RELAY_ADVERTISE_HOST").unwrap_or_else(|| "localhost".to_string());
        let port = port_number(non_empty_env("RELAY_PORT"), "RELAY_PORT")?.unwrap_or(4433);
        let inner_port = match port_number(non_empty_env("RELAY_INNER_PORT"), "RELAY_INNER_PORT")? {
            Some(inner_port) => inner_port,
            None => port.checked_add(1).ok_or_else(|| {
                anyhow::anyhow!("RELAY_INNER_PORT is required when RELAY_PORT is 65535")
            })?,
        };
        let public_host =
            non_empty_env("RELAY_PUBLIC_HOST").unwrap_or_else(|| format!("localhost:{port}"));
        let redis_url = non_empty_env("REDIS_URL");
        let drain_timeout = seconds(
            non_empty_env("RELAY_DRAIN_TIMEOUT_SECS"),
            "RELAY_DRAIN_TIMEOUT_SECS",
            Duration::from_secs(30),
        )?;
        let auth = AuthConfig::from_env_values(AuthEnv {
            vts_url: non_empty_env("AUTH_VTS_URL"),
            relay_token: non_empty_env("AUTH_RELAY_TOKEN"),
            max_token_ttl_seconds: non_empty_env("AUTH_MAX_TOKEN_TTL_SECONDS"),
            clock_leeway_seconds: non_empty_env("AUTH_CLOCK_LEEWAY_SECONDS"),
        })?;
        let disk_cache = DiskCacheConfig::from_env_values(
            non_empty_env("RELAY_DISK_CACHE_DIR"),
            non_empty_env("RELAY_DISK_CACHE_BYTES"),
        )?;

        Ok(Self {
            relay_id,
            advertise_host,
            public_host,
            port,
            inner_port,
            redis_url,
            drain_timeout,
            auth,
            disk_cache,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{AuthConfig, AuthEnv, DEFAULT_DISK_CACHE_BYTES, DiskCacheConfig, port_number};
    use crate::modules::auth::token_claims::ClaimPolicy;

    fn vts_env() -> AuthEnv {
        AuthEnv {
            vts_url: Some("http://vts/verify".to_string()),
            relay_token: Some("relay-jwt".to_string()),
            ..AuthEnv::default()
        }
    }

    #[test]
    fn vts_url_with_relay_token_uses_the_default_policy() {
        // Act
        let auth = AuthConfig::from_env_values(vts_env()).unwrap();

        // Assert
        assert_eq!(
            auth,
            AuthConfig {
                verify_url: "http://vts/verify".to_string(),
                relay_token: "relay-jwt".to_string(),
                claim_policy: ClaimPolicy::default(),
            }
        );
    }

    #[test]
    fn claim_policy_is_read_from_the_environment() {
        // Arrange
        let env = AuthEnv {
            max_token_ttl_seconds: Some("3600".to_string()),
            clock_leeway_seconds: Some("5".to_string()),
            ..vts_env()
        };

        // Act
        let auth = AuthConfig::from_env_values(env).unwrap();

        // Assert
        assert_eq!(auth.claim_policy.max_token_ttl, Duration::from_secs(3600));
        assert_eq!(auth.claim_policy.clock_leeway, Duration::from_secs(5));
    }

    #[test]
    fn non_numeric_policy_value_is_an_error() {
        // Arrange
        let env = AuthEnv {
            max_token_ttl_seconds: Some("1h".to_string()),
            ..vts_env()
        };

        // Act / Assert
        assert!(AuthConfig::from_env_values(env).is_err());
    }

    #[test]
    fn vts_url_without_relay_token_is_an_error() {
        // Arrange
        let env = AuthEnv {
            relay_token: None,
            ..vts_env()
        };

        // Act / Assert
        assert!(AuthConfig::from_env_values(env).is_err());
    }

    #[test]
    fn relay_token_without_vts_url_is_an_error() {
        // Arrange
        let env = AuthEnv {
            vts_url: None,
            ..vts_env()
        };

        // Act / Assert
        assert!(AuthConfig::from_env_values(env).is_err());
    }

    #[test]
    fn missing_configuration_refuses_to_start() {
        // Act / Assert
        assert!(AuthConfig::from_env_values(AuthEnv::default()).is_err());
    }

    #[test]
    fn disk_cache_is_off_without_a_directory() {
        // Act
        let config = DiskCacheConfig::from_env_values(None, Some("1024".to_string())).unwrap();

        // Assert
        assert_eq!(config, None);
    }

    #[test]
    fn disk_cache_defaults_its_byte_cap() {
        // Act
        let config = DiskCacheConfig::from_env_values(Some("/var/cache/relay".to_string()), None)
            .unwrap()
            .unwrap();

        // Assert
        assert_eq!(config.max_bytes, DEFAULT_DISK_CACHE_BYTES);
    }

    #[test]
    fn disk_cache_rejects_a_byte_cap_that_is_not_a_number() {
        // Act
        let result = DiskCacheConfig::from_env_values(
            Some("/var/cache/relay".to_string()),
            Some("10G".to_string()),
        );

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn port_that_is_not_a_number_names_the_variable() {
        // Act
        let error = port_number(Some("4433x".to_string()), "RELAY_INNER_PORT").unwrap_err();

        // Assert
        assert_eq!(
            error.to_string(),
            "RELAY_INNER_PORT must be a port number, got \"4433x\""
        );
    }
}
