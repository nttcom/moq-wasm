use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use broadcast::{MoqtTarget, token_app_id};

/// Where a stream goes, as the `moqt_service` settings say.
#[derive(Debug)]
pub(crate) struct PublishSettings {
    /// Without a token until `with_token`.
    pub(crate) target: MoqtTarget,
    /// Starts with the token's app ID once `with_token` ran.
    pub(crate) namespace: Vec<String>,
    pub(crate) token_file: Option<PathBuf>,
}

impl PublishSettings {
    /// Fails when the relay URL or the namespace is empty, unless exactly
    /// one of the token and the token file is set, or when `with_token`
    /// refuses the token. libobs gives an unset string setting as an empty
    /// string.
    pub(crate) fn new(
        server: &str,
        namespace: &str,
        bearer_token: &str,
        bearer_token_file: &str,
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
        if bearer_token.is_empty() == bearer_token_file.is_empty() {
            bail!(
                "the MoQT service needs exactly one of a token (`bearer_token`) and a token file (`bearer_token_file`)"
            );
        }
        let settings = Self {
            target: MoqtTarget {
                url: server.to_owned(),
                auth_token: None,
                verify_certificate: !insecure_skip_tls_verify,
            },
            namespace,
            token_file: (!bearer_token_file.is_empty()).then(|| PathBuf::from(bearer_token_file)),
        };
        if bearer_token.is_empty() {
            return Ok(settings);
        }
        settings.with_token(bearer_token.to_owned())
    }

    /// Gives the target `token` and puts the token's app ID in front of the
    /// namespace, unless the namespace starts with it already: a relay admits
    /// a token only into the namespaces of its app. Fails when `token` is not
    /// a JWT with an `appId` claim.
    pub(crate) fn with_token(mut self, token: String) -> Result<Self> {
        let app_id = token_app_id(&token)?;
        if self.namespace.first() != Some(&app_id) {
            self.namespace.insert(0, app_id);
        }
        self.target.auth_token = Some(token);
        Ok(self)
    }
}

/// The token in `path`, without the whitespace around it. Fails when the file
/// is unreadable or holds no token.
pub(crate) async fn read_token_file(path: &Path) -> Result<String> {
    let content = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("read the token file {}", path.display()))?;
    let token = content.trim();
    if token.is_empty() {
        bail!("the token file {} is empty", path.display());
    }
    Ok(token.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER: &str = "moqt://relay.example.com:4433";
    /// A JWT whose payload is `{"appId":"app"}`.
    const TOKEN: &str = "e30.eyJhcHBJZCI6ImFwcCJ9.signature";

    #[test]
    fn splits_the_namespace_and_verifies_the_certificate_by_default() {
        // Arrange / Act
        let settings = PublishSettings::new(
            "moqt://relay.example.com:4433",
            "/app/obs/live/",
            TOKEN,
            "",
            false,
        )
        .unwrap();

        // Assert
        assert_eq!(settings.namespace, ["app", "obs", "live"]);
        assert_eq!(settings.target.url, "moqt://relay.example.com:4433");
        assert_eq!(settings.target.auth_token.as_deref(), Some(TOKEN));
        assert_eq!(settings.token_file, None);
        assert!(settings.target.verify_certificate);
    }

    #[test]
    fn puts_the_token_s_app_id_in_front_of_a_namespace_without_it() {
        // Arrange / Act
        let settings = PublishSettings::new(SERVER, "obs/live", TOKEN, "", false).unwrap();

        // Assert
        assert_eq!(settings.namespace, ["app", "obs", "live"]);
    }

    #[test]
    fn a_token_read_from_the_token_file_names_the_app_as_well() {
        // Arrange
        let settings =
            PublishSettings::new(SERVER, "obs/live", "", "/run/obs/token", false).unwrap();

        // Act
        let settings = settings.with_token(TOKEN.to_owned()).unwrap();

        // Assert
        assert_eq!(settings.namespace, ["app", "obs", "live"]);
        assert_eq!(settings.target.auth_token.as_deref(), Some(TOKEN));
    }

    #[test]
    fn rejects_a_token_without_an_app_id() {
        // Act / Assert
        assert!(PublishSettings::new(SERVER, "obs/live", "opaque", "", false).is_err());
    }

    #[test]
    fn a_token_file_leaves_the_target_without_a_token() {
        // Arrange / Act
        let settings =
            PublishSettings::new(SERVER, "site1/obs", "", "/run/obs/token", false).unwrap();

        // Assert
        assert_eq!(settings.token_file, Some(PathBuf::from("/run/obs/token")));
        assert_eq!(settings.target.auth_token, None);
        assert_eq!(settings.namespace, ["site1", "obs"]);
    }

    #[test]
    fn skips_certificate_verification_on_request() {
        // Arrange / Act
        let settings =
            PublishSettings::new("https://127.0.0.1:4433", "room/camera", TOKEN, "", true).unwrap();

        // Assert
        assert!(!settings.target.verify_certificate);
    }

    #[test]
    fn rejects_a_missing_relay_url_or_namespace() {
        // Act / Assert
        assert!(PublishSettings::new("", "site1/obs", TOKEN, "", false).is_err());
        assert!(PublishSettings::new(SERVER, "//", TOKEN, "", false).is_err());
    }

    #[test]
    fn rejects_no_token_and_both_a_token_and_a_token_file() {
        // Act / Assert
        assert!(PublishSettings::new(SERVER, "site1/obs", "", "", false).is_err());
        assert!(PublishSettings::new(SERVER, "site1/obs", TOKEN, "/run/obs/token", false).is_err());
    }

    fn token_file(name: &str, content: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("obs-moqt-{}-{name}", std::process::id()));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[tokio::test]
    async fn reads_the_token_without_the_whitespace_around_it() {
        // Arrange
        let path = token_file("token", " jwt\n");

        // Act
        let token = read_token_file(&path).await;

        // Assert
        assert_eq!(token.unwrap(), "jwt");
    }

    #[tokio::test]
    async fn rejects_a_token_file_that_is_missing_or_holds_no_token() {
        // Arrange
        let blank = token_file("blank", "\n");
        let missing = std::env::temp_dir().join("obs-moqt-no-such-token-file");

        // Act / Assert
        assert!(read_token_file(&blank).await.is_err());
        assert!(read_token_file(&missing).await.is_err());
    }
}
