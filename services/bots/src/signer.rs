use std::time::Duration;

use anyhow::{Context, Result, ensure};
use app_registry::RegistryStore;
use auth_token::{Claims, TEMPORARY_APP_ID_PREFIX, sign_token};

const BOT_TOKEN_LIFETIME: Duration = Duration::from_secs(60 * 60);

/// Signs the client token a bot joins its room with, with the secret of the
/// room's app: a registered app's from the registry, read on every signature
/// so a rotated secret or a deleted app takes effect at the next renewal, or
/// the temporary app secret.
pub(crate) struct BotTokenSigner {
    registry: RegistryStore,
    temporary_app_secret: Option<String>,
}

impl BotTokenSigner {
    pub(crate) fn new(registry: RegistryStore, temporary_app_secret: Option<String>) -> Self {
        Self {
            registry,
            temporary_app_secret,
        }
    }

    /// A token that subscribes to the room `namespace_prefix` (`<app>/...`)
    /// and publishes only under `<room>/bots/<bot_type>`. `now` is Unix
    /// seconds. Fails when `namespace_prefix` is not a room of `app_id`, for
    /// an app the registry does not hold, a relay app, or a temporary app
    /// without the temporary app secret.
    pub(crate) async fn sign(
        &self,
        app_id: &str,
        namespace_prefix: &str,
        bot_type: &str,
        now: u64,
    ) -> Result<String> {
        let room = namespace_prefix
            .strip_prefix(app_id)
            .and_then(|rest| rest.strip_prefix('/'))
            .with_context(|| format!("{namespace_prefix} is not a room of {app_id}"))?;
        let claims = Claims::issued(
            app_id.to_string(),
            Some(format!("{room}/bots/{bot_type}")),
            Some(room.to_string()),
            now,
            BOT_TOKEN_LIFETIME,
        );
        let secret = if app_id.starts_with(TEMPORARY_APP_ID_PREFIX) {
            self.temporary_app_secret
                .clone()
                .context("no temporary app secret")?
        } else {
            let snapshot = self.registry.load().await?;
            let app = snapshot
                .apps
                .get(app_id)
                .with_context(|| format!("no app {app_id}"))?;
            ensure!(!app.is_relay, "{app_id} is a relay");
            app.secret.clone()
        };
        sign_token(&claims, &secret)
    }
}

#[cfg(test)]
mod tests {
    use app_registry::testing::TemporaryRegistryFile;
    use auth_token::{decode_unverified_payload, verify_signature};
    use serde_json::json;

    use super::*;

    fn signer(file: &TemporaryRegistryFile) -> BotTokenSigner {
        file.put(&json!([
            { "appId": "app", "secret": "app-secret", "isRelay": false },
            { "appId": "relay", "secret": "relay-secret", "isRelay": true },
        ]));
        BotTokenSigner::new(file.store(), Some("temporary-secret".into()))
    }

    #[tokio::test]
    async fn a_bot_subscribes_to_its_room_and_publishes_only_its_presence() {
        // Arrange
        let file = TemporaryRegistryFile::new();
        let signer = signer(&file);

        // Act
        let token = signer
            .sign("app", "app/room/a", "recorder", 100)
            .await
            .unwrap();

        // Assert
        verify_signature(&token, "app-secret").unwrap();
        let payload = decode_unverified_payload(&token).unwrap();
        assert_eq!(payload["subscribe"], "room/a");
        assert_eq!(payload["publish"], "room/a/bots/recorder");
        assert_eq!(payload["exp"], 3700);
    }

    #[tokio::test]
    async fn a_temporary_app_is_signed_with_the_temporary_secret() {
        // Arrange
        let file = TemporaryRegistryFile::new();
        let signer = signer(&file);

        // Act
        let token = signer
            .sign("temp-1", "temp-1/room", "chat", 100)
            .await
            .unwrap();

        // Assert
        verify_signature(&token, "temporary-secret").unwrap();
    }

    #[tokio::test]
    async fn an_unknown_app_a_relay_or_a_room_of_another_app_is_refused() {
        // Arrange
        let file = TemporaryRegistryFile::new();
        let signer = signer(&file);

        // Act / Assert
        assert!(
            signer
                .sign("other", "other/room", "chat", 100)
                .await
                .is_err()
        );
        assert!(
            signer
                .sign("relay", "relay/room", "chat", 100)
                .await
                .is_err()
        );
        assert!(signer.sign("app", "apple/room", "chat", 100).await.is_err());
        assert!(signer.sign("app", "app", "chat", 100).await.is_err());
    }
}
