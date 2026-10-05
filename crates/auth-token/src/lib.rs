use anyhow::{Context, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

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

/// Reads the claims without checking the signature; only the VTS can verify it.
pub fn decode_claims(token: &str) -> anyhow::Result<Claims> {
    let segments: Vec<&str> = token.split('.').collect();
    let [_, payload, _] = segments[..] else {
        bail!("token is not a compact JWS");
    };
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .context("token payload is not base64url")?;
    serde_json::from_slice(&payload).context("token payload is not a claims object")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn token_with_payload(payload: &str) -> String {
        format!(
            "{}.{}.sig",
            URL_SAFE_NO_PAD.encode(r#"{"alg":"HS256"}"#),
            URL_SAFE_NO_PAD.encode(payload)
        )
    }

    #[test]
    fn decodes_the_claims_of_a_compact_jws() {
        // Arrange
        let token = token_with_payload(
            r#"{"appId":"APP","publish":"site1/cam1","subscribe":"","iat":1,"exp":2,"custom":"ignored"}"#,
        );

        // Act
        let claims = decode_claims(&token).unwrap();

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
        let claims = decode_claims(&token_with_payload(r#"{"appId":"APP"}"#)).unwrap();

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

    #[test]
    fn rejects_a_token_without_three_segments() {
        // Act / Assert
        assert!(decode_claims("not-a-jwt").is_err());
    }

    #[test]
    fn rejects_a_payload_without_app_id() {
        // Act / Assert
        assert!(decode_claims(&token_with_payload(r#"{"publish":""}"#)).is_err());
    }

    #[test]
    fn rejects_a_payload_that_is_not_a_json_object() {
        // Act / Assert
        assert!(decode_claims(&token_with_payload(r#"["APP"]"#)).is_err());
    }
}
