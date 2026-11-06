use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use auth_token::{Claims, sign_token};

/// How long a token is good for: the relay's longest. When it runs out the
/// relay closes the session with EXPIRED_AUTH_TOKEN, which ends the broadcast
/// for every viewer until the client reconnects with a new token from its
/// provider, so the lifetime is as long as the relay allows.
pub(crate) const TOKEN_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// The app the translator serves: the broadcasts live under its id and every
/// session presents a token signed with its secret, the translator's own
/// (publish and subscribe the whole app) as well as the pages' (one broadcast).
pub(crate) struct AppAccess {
    pub(crate) app_id: String,
    secret: String,
}

impl AppAccess {
    pub(crate) fn new(app_id: String, secret: String) -> Self {
        Self { app_id, secret }
    }

    /// `publish` and `subscribe` are paths under the app, `""` for all of it;
    /// `None` grants nothing for that operation.
    pub(crate) fn issue(&self, publish: Option<&str>, subscribe: Option<&str>) -> Result<String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("clock before 1970")?
            .as_secs();
        let claims = Claims::issued(
            self.app_id.clone(),
            publish.map(str::to_string),
            subscribe.map(str::to_string),
            now,
            TOKEN_LIFETIME,
        );
        sign_token(&claims, &self.secret)
    }

    pub(crate) fn translator_token(&self) -> Result<String> {
        self.issue(Some(""), Some(""))
    }
}

#[cfg(test)]
mod tests {
    use auth_token::{decode_unverified_payload, verify_signature};

    use super::*;

    #[test]
    fn a_page_token_is_signed_with_the_secret_and_scoped_to_its_broadcast() {
        // Arrange
        let access = AppAccess::new("app-1".to_string(), "s3cret".to_string());

        // Act
        let token = access.issue(None, Some("brave-otter")).unwrap();

        // Assert
        verify_signature(&token, "s3cret").unwrap();
        let payload = decode_unverified_payload(&token).unwrap();
        assert_eq!(payload["appId"], "app-1");
        assert_eq!(payload["subscribe"], "brave-otter");
        assert!(payload.get("publish").is_none());
        assert_eq!(
            payload["exp"].as_u64().unwrap() - payload["iat"].as_u64().unwrap(),
            TOKEN_LIFETIME.as_secs()
        );
    }

    #[test]
    fn the_translator_token_covers_the_whole_app() {
        // Arrange
        let access = AppAccess::new("app-1".to_string(), "s3cret".to_string());

        // Act
        let token = access.translator_token().unwrap();

        // Assert
        let payload = decode_unverified_payload(&token).unwrap();
        assert_eq!(payload["publish"], "");
        assert_eq!(payload["subscribe"], "");
    }
}
