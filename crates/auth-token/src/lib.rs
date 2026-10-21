mod jwt;

use serde::{Deserialize, Serialize};

pub use jwt::{SignatureError, decode_unverified_payload, sign_token, verify_signature};

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
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn deserialisation_reads_every_claim_and_ignores_unknown_ones() {
        // Arrange
        let payload = json!({
            "appId": "APP",
            "publish": "site1/cam1",
            "subscribe": "",
            "iat": 1,
            "exp": 2,
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
        };

        // Act / Assert
        assert_eq!(
            serde_json::to_value(&claims).unwrap(),
            json!({ "appId": "APP", "publish": "", "iat": 1, "exp": 2 })
        );
    }
}
