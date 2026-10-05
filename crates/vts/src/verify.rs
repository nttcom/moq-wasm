use serde_json::{Map, Value};

use crate::{apps::Apps, jwt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RejectReason {
    MalformedToken,
    UnknownApp,
    InvalidSignature,
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

pub(crate) fn verify_token(token: &str, apps: &Apps) -> Result<VerifiedToken, RejectReason> {
    let claims = jwt::decode_claims(token).ok_or(RejectReason::MalformedToken)?;
    let app_id = claims
        .get("appId")
        .and_then(Value::as_str)
        .filter(|app_id| !app_id.is_empty())
        .ok_or(RejectReason::MalformedToken)?;
    let app = apps.get(app_id).ok_or(RejectReason::UnknownApp)?;
    jwt::verify_signature(token, &app.secret)?;
    Ok(VerifiedToken {
        app_id: app_id.to_owned(),
        is_relay: app.is_relay,
        claims,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::test_support::{
        APP_ID, APP_SECRET, NOW, RELAY_ID, RELAY_SECRET, app_claims, test_apps, token,
    };

    #[test]
    fn valid_token_yields_the_app_id_the_relay_flag_and_the_raw_claims() {
        // Arrange
        let apps = test_apps();
        let mut claims = app_claims();
        claims["publish"] = json!("site1/cam1");
        let signed = token(&claims, APP_SECRET);

        // Act
        let verified = verify_token(&signed, &apps).unwrap();

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
        let verified = verify_token(&signed, &apps).unwrap();

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
        let verified = verify_token(&expired, &apps).unwrap();

        // Assert
        assert_eq!(verified.claims["exp"], json!(NOW - 7200 + 60));
    }

    #[test]
    fn token_that_is_not_a_jwt_is_malformed() {
        // Act / Assert
        assert_eq!(
            verify_token("not-a-jwt", &test_apps()),
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
            verify_token(&signed, &test_apps()),
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
            verify_token(&signed, &test_apps()),
            Err(RejectReason::UnknownApp)
        );
    }

    #[test]
    fn token_signed_with_another_secret_is_rejected() {
        // Arrange
        let signed = token(&app_claims(), "wrong-secret");

        // Act / Assert
        assert_eq!(
            verify_token(&signed, &test_apps()),
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
            verify_token(&unsigned, &test_apps()),
            Err(RejectReason::InvalidSignature)
        );
    }
}
