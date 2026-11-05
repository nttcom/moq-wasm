use app_registry::Apps;
use auth_token::{
    SignatureError, TEMPORARY_APP_ID_PREFIX, decode_unverified_payload, verify_signature,
};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RejectReason {
    MalformedToken,
    UnknownApp,
    InvalidSignature,
}

impl From<SignatureError> for RejectReason {
    fn from(error: SignatureError) -> Self {
        match error {
            SignatureError::MalformedToken => Self::MalformedToken,
            _ => Self::InvalidSignature,
        }
    }
}

impl RejectReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::MalformedToken => "malformed_token",
            Self::UnknownApp => "unknown_app",
            Self::InvalidSignature => "invalid_signature",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VerifiedToken {
    pub(crate) app_id: String,
    pub(crate) is_relay: bool,
    pub(crate) claims: Map<String, Value>,
}

pub(crate) fn verify_token(
    token: &str,
    apps: &Apps,
    temporary_secret: Option<&str>,
) -> Result<VerifiedToken, RejectReason> {
    let claims = decode_unverified_payload(token).ok_or(RejectReason::MalformedToken)?;
    let app_id = claims
        .get("appId")
        .and_then(Value::as_str)
        .filter(|app_id| !app_id.is_empty())
        .ok_or(RejectReason::MalformedToken)?;
    let (secret, is_relay) = match temporary_secret {
        Some(secret) if app_id.starts_with(TEMPORARY_APP_ID_PREFIX) => (secret, false),
        _ => {
            let app = apps.get(app_id).ok_or(RejectReason::UnknownApp)?;
            (app.secret.as_str(), app.is_relay)
        }
    };
    verify_signature(token, secret)?;
    Ok(VerifiedToken {
        app_id: app_id.to_owned(),
        is_relay,
        claims,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::test_support::{
        APP_ID, APP_SECRET, NOW, RELAY_ID, RELAY_SECRET, TEMPORARY_APP_ID, TEMPORARY_SECRET,
        app_claims, temporary_app_claims, test_apps, token,
    };

    #[test]
    fn valid_token_yields_the_app_id_the_relay_flag_and_the_raw_claims() {
        // Arrange
        let apps = test_apps();
        let mut claims = app_claims();
        claims["publish"] = json!("site1/cam1");
        let signed = token(&claims, APP_SECRET);

        // Act
        let verified = verify_token(&signed, &apps, None).unwrap();

        // Assert
        assert_eq!(
            verified,
            VerifiedToken {
                app_id: APP_ID.into(),
                is_relay: false,
                claims: claims.as_object().unwrap().clone(),
            }
        );
    }

    #[test]
    fn relay_app_is_flagged_as_relay() {
        // Arrange
        let apps = test_apps();
        let signed = token(
            &json!({ "appId": RELAY_ID, "publish": "", "subscribe": "", "iat": NOW, "exp": NOW + 60 }),
            RELAY_SECRET,
        );

        // Act
        let verified = verify_token(&signed, &apps, None).unwrap();

        // Assert
        assert!(verified.is_relay);
    }

    #[test]
    fn time_claims_are_passed_through_not_judged() {
        // Arrange
        let apps = test_apps();
        let mut claims = app_claims();
        claims["iat"] = json!(NOW - 7200);
        claims["exp"] = json!(NOW - 7200 + 60);
        let expired = token(&claims, APP_SECRET);

        // Act
        let verified = verify_token(&expired, &apps, None).unwrap();

        // Assert
        assert_eq!(verified.claims["exp"], json!(NOW - 7200 + 60));
    }

    #[test]
    fn token_that_is_not_a_jwt_is_malformed() {
        // Act / Assert
        assert_eq!(
            verify_token("not-a-jwt", &test_apps(), None),
            Err(RejectReason::MalformedToken)
        );
    }

    #[test]
    fn token_without_app_id_is_malformed() {
        // Arrange
        let signed = token(
            &json!({ "publish": "", "iat": NOW, "exp": NOW + 60 }),
            APP_SECRET,
        );

        // Act / Assert
        assert_eq!(
            verify_token(&signed, &test_apps(), None),
            Err(RejectReason::MalformedToken)
        );
    }

    #[test]
    fn unknown_app_id_is_rejected_before_the_signature_is_checked() {
        // Arrange
        let mut claims = app_claims();
        claims["appId"] = json!("NOBODY");
        let signed = token(&claims, "any-secret");

        // Act / Assert
        assert_eq!(
            verify_token(&signed, &test_apps(), None),
            Err(RejectReason::UnknownApp)
        );
    }

    #[test]
    fn token_signed_with_another_secret_is_rejected() {
        // Arrange
        let signed = token(&app_claims(), "wrong-secret");

        // Act / Assert
        assert_eq!(
            verify_token(&signed, &test_apps(), None),
            Err(RejectReason::InvalidSignature)
        );
    }

    #[test]
    fn unsigned_token_is_rejected() {
        // Arrange
        let signed = token(&app_claims(), APP_SECRET);
        let signing_input = signed.rsplit_once('.').unwrap().0;
        let unsigned = format!("{signing_input}.");

        // Act / Assert
        assert_eq!(
            verify_token(&unsigned, &test_apps(), None),
            Err(RejectReason::InvalidSignature)
        );
    }

    #[test]
    fn temporary_app_is_verified_with_the_temporary_secret_as_a_client() {
        // Arrange
        let claims = temporary_app_claims();
        let signed = token(&claims, TEMPORARY_SECRET);

        // Act
        let verified = verify_token(&signed, &test_apps(), Some(TEMPORARY_SECRET)).unwrap();

        // Assert
        assert_eq!(
            verified,
            VerifiedToken {
                app_id: TEMPORARY_APP_ID.into(),
                is_relay: false,
                claims: claims.as_object().unwrap().clone(),
            }
        );
    }

    #[test]
    fn temporary_app_signed_with_another_secret_is_rejected() {
        // Arrange
        let signed = token(&temporary_app_claims(), "wrong-secret");

        // Act / Assert
        assert_eq!(
            verify_token(&signed, &test_apps(), Some(TEMPORARY_SECRET)),
            Err(RejectReason::InvalidSignature)
        );
    }

    #[test]
    fn temporary_app_is_unknown_without_the_temporary_secret() {
        // Arrange
        let signed = token(&temporary_app_claims(), TEMPORARY_SECRET);

        // Act / Assert
        assert_eq!(
            verify_token(&signed, &test_apps(), None),
            Err(RejectReason::UnknownApp)
        );
    }

    #[test]
    fn registered_app_is_not_verified_with_the_temporary_secret() {
        // Arrange
        let signed = token(&app_claims(), TEMPORARY_SECRET);

        // Act / Assert
        assert_eq!(
            verify_token(&signed, &test_apps(), Some(TEMPORARY_SECRET)),
            Err(RejectReason::InvalidSignature)
        );
    }
}
