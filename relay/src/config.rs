use std::time::Duration;

use anyhow::bail;

use crate::modules::auth::token_claims::ClaimPolicy;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthConfig {
    Disabled,
    Vts {
        verify_url: String,
        relay_token: String,
        claim_policy: ClaimPolicy,
    },
}

#[derive(Default)]
struct AuthEnv {
    vts_url: Option<String>,
    relay_token: Option<String>,
    disabled: Option<String>,
    max_token_ttl_seconds: Option<String>,
    clock_leeway_seconds: Option<String>,
}

fn seconds(value: Option<String>, name: &str, default: Duration) -> anyhow::Result<Duration> {
    match value {
        None => Ok(default),
        Some(text) => text
            .parse::<u64>()
            .map(Duration::from_secs)
            .map_err(|_| anyhow::anyhow!("{name} must be a non-negative integer, got {text:?}")),
    }
}

impl AuthConfig {
    fn from_env_values(env: AuthEnv) -> anyhow::Result<Self> {
        if let Some(verify_url) = env.vts_url {
            let Some(relay_token) = env.relay_token else {
                bail!("AUTH_RELAY_TOKEN is required when AUTH_VTS_URL is set");
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
            return Ok(Self::Vts {
                verify_url,
                relay_token,
                claim_policy,
            });
        }
        if env.disabled.as_deref() == Some("true") {
            return Ok(Self::Disabled);
        }
        bail!(
            "authentication is not configured: set AUTH_VTS_URL and AUTH_RELAY_TOKEN, \
             or AUTH_DISABLED=true to run without authentication"
        )
    }

    pub(crate) fn relay_token(&self) -> Option<String> {
        match self {
            Self::Disabled => None,
            Self::Vts { relay_token, .. } => Some(relay_token.clone()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RelayConfig {
    pub relay_id: String,
    pub advertise_host: String,
    pub port: u16,
    pub inner_port: u16,
    pub redis_url: Option<String>,
    pub auth: AuthConfig,
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
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
        let auth = AuthConfig::from_env_values(AuthEnv {
            vts_url: non_empty_env("AUTH_VTS_URL"),
            relay_token: non_empty_env("AUTH_RELAY_TOKEN"),
            disabled: non_empty_env("AUTH_DISABLED"),
            max_token_ttl_seconds: non_empty_env("AUTH_MAX_TOKEN_TTL_SECONDS"),
            clock_leeway_seconds: non_empty_env("AUTH_CLOCK_LEEWAY_SECONDS"),
        })?;

        Ok(Self {
            relay_id,
            advertise_host,
            port,
            inner_port,
            redis_url,
            auth,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{AuthConfig, AuthEnv};
    use crate::modules::auth::token_claims::ClaimPolicy;

    fn vts_env() -> AuthEnv {
        AuthEnv {
            vts_url: Some("http://vts/verify".to_string()),
            relay_token: Some("relay-jwt".to_string()),
            ..AuthEnv::default()
        }
    }

    #[test]
    fn vts_url_with_relay_token_enables_verification_with_default_policy() {
        // Act
        let auth = AuthConfig::from_env_values(vts_env()).unwrap();

        // Assert
        assert_eq!(
            auth,
            AuthConfig::Vts {
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
        let AuthConfig::Vts { claim_policy, .. } = AuthConfig::from_env_values(env).unwrap() else {
            panic!("expected Vts");
        };

        // Assert
        assert_eq!(claim_policy.max_token_ttl, Duration::from_secs(3600));
        assert_eq!(claim_policy.clock_leeway, Duration::from_secs(5));
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
    fn explicit_opt_out_disables_authentication() {
        // Act
        let auth = AuthConfig::from_env_values(AuthEnv {
            disabled: Some("true".to_string()),
            ..AuthEnv::default()
        })
        .unwrap();

        // Assert
        assert_eq!(auth, AuthConfig::Disabled);
    }

    #[test]
    fn missing_configuration_refuses_to_start() {
        // Act / Assert
        assert!(AuthConfig::from_env_values(AuthEnv::default()).is_err());
    }

    #[test]
    fn vts_url_takes_precedence_over_opt_out() {
        // Act
        let auth = AuthConfig::from_env_values(AuthEnv {
            disabled: Some("true".to_string()),
            ..vts_env()
        })
        .unwrap();

        // Assert
        assert!(matches!(auth, AuthConfig::Vts { .. }));
    }
}
