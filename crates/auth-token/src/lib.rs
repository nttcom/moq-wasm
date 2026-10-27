mod jwt;
mod lifetime;

use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use jwt::{SignatureError, decode_unverified_payload, sign_token, verify_signature};
pub use lifetime::parse_lifetime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Claims {
    pub app_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscribe: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iat: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<u64>,
    /// The `RELAY_ID` of the relay presenting this token. The accepting relay
    /// reads it only from a relay app's token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_id: Option<String>,
}

impl Claims {
    /// `now` is Unix seconds.
    pub fn issued(
        app_id: String,
        publish: Option<String>,
        subscribe: Option<String>,
        now: u64,
        lifetime: Duration,
    ) -> Self {
        Self {
            app_id,
            publish,
            subscribe,
            iat: Some(now),
            exp: Some(now + lifetime.as_secs()),
            relay_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn issued_claims_expire_the_lifetime_after_now() {
        // Act
        let claims = Claims::issued(
            "APP".into(),
            Some("cam1".into()),
            None,
            100,
            Duration::from_secs(60),
        );

        // Assert
        assert_eq!((claims.iat, claims.exp), (Some(100), Some(160)));
    }

    #[test]
    fn deserialisation_reads_every_claim_and_ignores_unknown_ones() {
        // Arrange
        let payload = json!({
            "appId": "APP",
            "publish": "site1/cam1",
            "subscribe": "",
            "iat": 1,
            "exp": 2,
            "relayId": "relay-a",
            "custom": "ignored",
        });

        // Act
        let claims: Claims = serde_json::from_value(payload).unwrap();

        // Assert
        assert_eq!(
            claims,
            Claims {
                app_id: "APP".into(),
                publish: Some("site1/cam1".into()),
                subscribe: Some(String::new()),
                iat: Some(1),
                exp: Some(2),
                relay_id: Some("relay-a".into()),
            }
        );
    }

    #[test]
    fn optional_claims_default_to_none() {
        // Act
        let claims: Claims = serde_json::from_value(json!({ "appId": "APP" })).unwrap();

        // Assert
        assert_eq!(
            claims,
            Claims {
                app_id: "APP".into(),
                publish: None,
                subscribe: None,
                iat: None,
                exp: None,
                relay_id: None,
            }
        );
    }

    #[test]
    fn deserialisation_rejects_a_payload_without_app_id() {
        // Act / Assert
        assert!(serde_json::from_value::<Claims>(json!({ "publish": "" })).is_err());
    }

    #[test]
    fn serialisation_omits_absent_optional_claims() {
        // Arrange
        let claims = Claims {
            app_id: "APP".into(),
            publish: Some(String::new()),
            subscribe: None,
            iat: Some(1),
            exp: Some(2),
            relay_id: None,
        };

        // Act / Assert
        assert_eq!(
            serde_json::to_value(&claims).unwrap(),
            json!({ "appId": "APP", "publish": "", "iat": 1, "exp": 2 })
        );
    }
}
