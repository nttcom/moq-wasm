use thiserror::Error;

use crate::{TEMPORARY_APP_ID_PREFIX, decode_unverified_payload};

#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum TemporaryAppTokenError {
    #[error("not a token of a temporary app")]
    NotTemporaryApp,
    #[error("the token has expired or has no expiry")]
    Expired,
}

/// The `appId` of a token of a temporary app that has not expired at `now`
/// (Unix seconds). The signature is not verified: verify it first, or leave
/// it to whoever accepts the token.
pub fn temporary_app_id(token: &str, now: u64) -> Result<String, TemporaryAppTokenError> {
    let payload =
        decode_unverified_payload(token).ok_or(TemporaryAppTokenError::NotTemporaryApp)?;
    let app_id = payload
        .get("appId")
        .and_then(|app_id| app_id.as_str())
        .filter(|app_id| app_id.starts_with(TEMPORARY_APP_ID_PREFIX))
        .ok_or(TemporaryAppTokenError::NotTemporaryApp)?;
    let expires_at = payload.get("exp").and_then(|exp| exp.as_u64());
    if expires_at.is_none_or(|expires_at| expires_at <= now) {
        return Err(TemporaryAppTokenError::Expired);
    }
    Ok(app_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Claims, sign_token};

    const NOW: u64 = 1_800_000_000;

    fn token(app_id: &str, exp: Option<u64>) -> String {
        let claims = Claims {
            app_id: app_id.to_string(),
            publish: Some(String::new()),
            bots: None,
            subscribe: Some(String::new()),
            iat: None,
            exp,
            relay_id: None,
        };
        sign_token(&claims, "s3cret").unwrap()
    }

    #[test]
    fn a_live_token_of_a_temporary_app_names_the_app() {
        // Act
        let app_id = temporary_app_id(&token("temp-3f9a", Some(NOW + 1)), NOW);

        // Assert
        assert_eq!(app_id, Ok("temp-3f9a".to_string()));
    }

    #[test]
    fn a_token_of_a_registered_app_is_refused() {
        // Act
        let app_id = temporary_app_id(&token("app-1", Some(NOW + 1)), NOW);

        // Assert
        assert_eq!(app_id, Err(TemporaryAppTokenError::NotTemporaryApp));
    }

    #[test]
    fn text_that_is_not_a_token_is_refused() {
        // Act / Assert
        assert_eq!(
            temporary_app_id("not-a-token", NOW),
            Err(TemporaryAppTokenError::NotTemporaryApp)
        );
    }

    #[test]
    fn an_expired_token_or_one_without_an_expiry_is_refused() {
        // Act / Assert
        for exp in [Some(NOW), None] {
            assert_eq!(
                temporary_app_id(&token("temp-3f9a", exp), NOW),
                Err(TemporaryAppTokenError::Expired)
            );
        }
    }
}
