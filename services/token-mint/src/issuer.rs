use std::time::Duration;

use anyhow::Context;
use auth_token::{
    Claims, TEMPORARY_APP_ID_PREFIX, decode_unverified_payload, sign_token, verify_signature,
};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Serialize;

pub(crate) const TOKEN_LIFETIME: Duration = Duration::from_secs(60 * 60);

#[derive(Debug)]
pub(crate) enum RefreshError {
    /// Not a temporary app token signed with this issuer's secret.
    NotIssuedHere,
    Expired,
    Internal(anyhow::Error),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IssuedToken {
    pub(crate) token: String,
    pub(crate) app_id: String,
    /// Unix seconds.
    pub(crate) expires_at: u64,
}

/// Signs the tokens of temporary apps with the secret the VTS verifies them with.
pub struct TokenMint {
    secret: String,
}

impl TokenMint {
    pub fn new(secret: String) -> Self {
        Self { secret }
    }

    /// A token for a new temporary app, allowed to publish and subscribe
    /// anywhere under the app's root. `now` is Unix seconds.
    pub(crate) fn issue(&self, now: u64) -> anyhow::Result<IssuedToken> {
        self.sign(random_app_id()?, now)
    }

    pub(crate) fn refresh(&self, token: &str, now: u64) -> Result<IssuedToken, RefreshError> {
        verify_signature(token, &self.secret).map_err(|_| RefreshError::NotIssuedHere)?;
        let claims = decode_unverified_payload(token).ok_or(RefreshError::NotIssuedHere)?;
        let app_id = claims
            .get("appId")
            .and_then(|value| value.as_str())
            .filter(|app_id| app_id.starts_with(TEMPORARY_APP_ID_PREFIX))
            .ok_or(RefreshError::NotIssuedHere)?;
        let exp = claims.get("exp").and_then(|value| value.as_u64());
        if exp.is_none_or(|exp| exp <= now) {
            return Err(RefreshError::Expired);
        }
        self.sign(app_id.to_owned(), now)
            .map_err(RefreshError::Internal)
    }

    fn sign(&self, app_id: String, now: u64) -> anyhow::Result<IssuedToken> {
        let claims = Claims::issued(
            app_id.clone(),
            Some(String::new()),
            Some(String::new()),
            now,
            TOKEN_LIFETIME,
        );
        Ok(IssuedToken {
            token: sign_token(&claims, &self.secret)?,
            app_id,
            expires_at: now + TOKEN_LIFETIME.as_secs(),
        })
    }
}

fn random_app_id() -> anyhow::Result<String> {
    let mut bytes = [0; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .ok()
        .context("the system random generator failed")?;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{TEMPORARY_APP_ID_PREFIX}{hex}"))
}

#[cfg(test)]
mod tests {
    use auth_token::sign_token;
    use serde_json::json;

    use super::*;

    const SECRET: &str = "temporary-secret";
    const NOW: u64 = 1_759_600_000;

    fn issuer() -> TokenMint {
        TokenMint::new(SECRET.into())
    }

    fn claims_of(token: &str) -> serde_json::Value {
        serde_json::Value::Object(decode_unverified_payload(token).unwrap())
    }

    #[test]
    fn issue_signs_a_one_hour_token_for_the_whole_root_of_a_new_temporary_app() {
        // Act
        let issued = issuer().issue(NOW).unwrap();

        // Assert
        verify_signature(&issued.token, SECRET).unwrap();
        assert!(issued.app_id.starts_with(TEMPORARY_APP_ID_PREFIX));
        assert_eq!(issued.expires_at, NOW + 3600);
        assert_eq!(
            claims_of(&issued.token),
            json!({
                "appId": issued.app_id,
                "publish": "",
                "subscribe": "",
                "iat": NOW,
                "exp": NOW + 3600,
            })
        );
    }

    #[test]
    fn issue_gives_every_token_another_app() {
        // Arrange
        let issuer = issuer();

        // Act
        let (first, second) = (issuer.issue(NOW).unwrap(), issuer.issue(NOW).unwrap());

        // Assert
        assert_ne!(first.app_id, second.app_id);
    }

    #[test]
    fn refresh_keeps_the_app_and_restarts_the_hour() {
        // Arrange
        let issuer = issuer();
        let issued = issuer.issue(NOW).unwrap();

        // Act
        let refreshed = issuer.refresh(&issued.token, NOW + 3000).unwrap();

        // Assert
        assert_eq!(refreshed.app_id, issued.app_id);
        assert_eq!(refreshed.expires_at, NOW + 3000 + 3600);
    }

    #[test]
    fn refresh_rejects_an_expired_token() {
        // Arrange
        let issuer = issuer();
        let issued = issuer.issue(NOW).unwrap();

        // Act / Assert
        assert!(matches!(
            issuer.refresh(&issued.token, NOW + 3600),
            Err(RefreshError::Expired)
        ));
    }

    #[test]
    fn refresh_rejects_a_token_signed_with_another_secret() {
        // Arrange
        let token = sign_token(
            &json!({ "appId": "temp-x", "publish": "", "subscribe": "", "iat": NOW, "exp": NOW + 60 }),
            "another-secret",
        )
        .unwrap();

        // Act / Assert
        assert!(matches!(
            issuer().refresh(&token, NOW),
            Err(RefreshError::NotIssuedHere)
        ));
    }

    #[test]
    fn refresh_rejects_an_app_that_is_not_temporary() {
        // Arrange
        let token = sign_token(
            &json!({ "appId": "registered", "publish": "", "subscribe": "", "iat": NOW, "exp": NOW + 60 }),
            SECRET,
        )
        .unwrap();

        // Act / Assert
        assert!(matches!(
            issuer().refresh(&token, NOW),
            Err(RefreshError::NotIssuedHere)
        ));
    }
}
