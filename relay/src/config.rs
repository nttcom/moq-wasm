use anyhow::bail;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthConfig {
    Disabled,
    Vts {
        verify_url: String,
        relay_token: String,
    },
}

impl AuthConfig {
    fn from_values(
        vts_url: Option<String>,
        relay_token: Option<String>,
        disabled: Option<String>,
    ) -> anyhow::Result<Self> {
        if let Some(verify_url) = vts_url {
            let Some(relay_token) = relay_token else {
                bail!("AUTH_RELAY_TOKEN is required when AUTH_VTS_URL is set");
            };
            return Ok(Self::Vts {
                verify_url,
                relay_token,
            });
        }
        if disabled.as_deref() == Some("true") {
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
        let auth = AuthConfig::from_values(
            non_empty_env("AUTH_VTS_URL"),
            non_empty_env("AUTH_RELAY_TOKEN"),
            non_empty_env("AUTH_DISABLED"),
        )?;

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
    use super::AuthConfig;

    #[test]
    fn vts_url_with_relay_token_enables_verification() {
        // Act
        let auth = AuthConfig::from_values(
            Some("http://vts/verify".to_string()),
            Some("relay-jwt".to_string()),
            None,
        )
        .unwrap();

        // Assert
        assert_eq!(
            auth,
            AuthConfig::Vts {
                verify_url: "http://vts/verify".to_string(),
                relay_token: "relay-jwt".to_string(),
            }
        );
    }

    #[test]
    fn vts_url_without_relay_token_is_an_error() {
        // Act
        let result = AuthConfig::from_values(Some("http://vts/verify".to_string()), None, None);

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn explicit_opt_out_disables_authentication() {
        // Act
        let auth = AuthConfig::from_values(None, None, Some("true".to_string())).unwrap();

        // Assert
        assert_eq!(auth, AuthConfig::Disabled);
    }

    #[test]
    fn missing_configuration_refuses_to_start() {
        // Act
        let result = AuthConfig::from_values(None, None, None);

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn vts_url_takes_precedence_over_opt_out() {
        // Act
        let auth = AuthConfig::from_values(
            Some("http://vts/verify".to_string()),
            Some("relay-jwt".to_string()),
            Some("true".to_string()),
        )
        .unwrap();

        // Assert
        assert!(matches!(auth, AuthConfig::Vts { .. }));
    }
}
