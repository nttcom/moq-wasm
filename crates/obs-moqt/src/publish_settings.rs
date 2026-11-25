use anyhow::{Result, bail};
use broadcast::MoqtTarget;

/// Where a stream goes, as the `moqt_service` settings say.
#[derive(Debug)]
pub(crate) struct PublishSettings {
    pub(crate) target: MoqtTarget,
    pub(crate) namespace: Vec<String>,
}

impl PublishSettings {
    /// Fails when the relay URL, the namespace, or the token is empty. libobs
    /// gives an unset string setting as an empty string.
    pub(crate) fn new(
        server: &str,
        namespace: &str,
        bearer_token: &str,
        insecure_skip_tls_verify: bool,
    ) -> Result<Self> {
        if server.is_empty() {
            bail!("the MoQT service has no relay URL (`server`)");
        }
        let namespace: Vec<String> = namespace
            .split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect();
        if namespace.is_empty() {
            bail!("the MoQT service has no track namespace (`namespace`)");
        }
        if bearer_token.is_empty() {
            bail!("the MoQT service has no token (`bearer_token`)");
        }
        Ok(Self {
            target: MoqtTarget {
                url: server.to_owned(),
                auth_token: Some(bearer_token.to_owned()),
                verify_certificate: !insecure_skip_tls_verify,
            },
            namespace,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_the_namespace_and_verifies_the_certificate_by_default() {
        // Arrange / Act
        let settings = PublishSettings::new(
            "moqt://relay.example.com:4433",
            "/site1/obs/live/",
            "jwt",
            false,
        )
        .unwrap();

        // Assert
        assert_eq!(settings.namespace, ["site1", "obs", "live"]);
        assert_eq!(settings.target.url, "moqt://relay.example.com:4433");
        assert_eq!(settings.target.auth_token.as_deref(), Some("jwt"));
        assert!(settings.target.verify_certificate);
    }

    #[test]
    fn skips_certificate_verification_on_request() {
        // Arrange / Act
        let settings =
            PublishSettings::new("https://127.0.0.1:4433", "room/camera", "jwt", true).unwrap();

        // Assert
        assert!(!settings.target.verify_certificate);
    }

    #[test]
    fn rejects_a_missing_relay_url_namespace_or_token() {
        // Arrange
        let server = "moqt://relay.example.com:4433";

        // Act / Assert
        assert!(PublishSettings::new("", "site1/obs", "jwt", false).is_err());
        assert!(PublishSettings::new(server, "//", "jwt", false).is_err());
        assert!(PublishSettings::new(server, "site1/obs", "", false).is_err());
    }
}
