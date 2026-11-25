use anyhow::{Result, bail};
use broadcast::MoqtTarget;

/// Where a stream goes, as the `moqt_service` settings say.
#[derive(Debug)]
pub(crate) struct PublishSettings {
    pub(crate) target: MoqtTarget,
    pub(crate) namespace: Vec<String>,
}

impl PublishSettings {
    /// Fails when the relay URL or the namespace is empty. libobs gives an
    /// unset string setting as an empty string.
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
        Ok(Self {
            target: MoqtTarget {
                url: server.to_owned(),
                auth_token: (!bearer_token.is_empty()).then(|| bearer_token.to_owned()),
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
            "/anon/obs/live/",
            "",
            false,
        )
        .unwrap();

        // Assert
        assert_eq!(settings.namespace, ["anon", "obs", "live"]);
        assert_eq!(settings.target.url, "moqt://relay.example.com:4433");
        assert_eq!(settings.target.auth_token, None);
        assert!(settings.target.verify_certificate);
    }

    #[test]
    fn carries_the_token_and_the_request_to_skip_certificate_verification() {
        // Arrange / Act
        let settings =
            PublishSettings::new("https://127.0.0.1:4433", "room/camera", "jwt", true).unwrap();

        // Assert
        assert_eq!(settings.target.auth_token.as_deref(), Some("jwt"));
        assert!(!settings.target.verify_certificate);
    }

    #[test]
    fn rejects_a_missing_relay_url_or_namespace() {
        // Act / Assert
        assert!(PublishSettings::new("", "anon/obs", "", false).is_err());
        assert!(PublishSettings::new("moqt://relay.example.com:4433", "//", "", false).is_err());
    }
}
