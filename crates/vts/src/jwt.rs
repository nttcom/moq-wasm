use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::hmac;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::verify::RejectReason;

pub(crate) fn sign_token(claims: &impl Serialize, secret: &str) -> anyhow::Result<String> {
    let payload = serde_json::to_vec(claims)?;
    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(r#"{"alg":"HS256"}"#),
        URL_SAFE_NO_PAD.encode(payload)
    );
    let tag = hmac::sign(&hmac_key(secret), signing_input.as_bytes());
    Ok(format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(tag.as_ref())
    ))
}

pub(crate) fn decode_claims(token: &str) -> Option<Map<String, Value>> {
    let [_, payload, _] = split_compact(token)?;
    decode_json_object(payload)
}

pub(crate) fn verify_signature(token: &str, secret: &str) -> Result<(), RejectReason> {
    let [header_segment, payload_segment, signature_segment] =
        split_compact(token).ok_or(RejectReason::MalformedToken)?;
    let header = decode_json_object(header_segment).ok_or(RejectReason::MalformedToken)?;
    let alg = header
        .get("alg")
        .and_then(Value::as_str)
        .ok_or(RejectReason::MalformedToken)?;
    if alg != "HS256" {
        return Err(RejectReason::InvalidSignature);
    }
    let signature = URL_SAFE_NO_PAD
        .decode(signature_segment)
        .map_err(|_| RejectReason::MalformedToken)?;
    let signing_input = &token[..header_segment.len() + 1 + payload_segment.len()];
    hmac::verify(&hmac_key(secret), signing_input.as_bytes(), &signature)
        .map_err(|_| RejectReason::InvalidSignature)
}

fn hmac_key(secret: &str) -> hmac::Key {
    hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes())
}

fn split_compact(token: &str) -> Option<[&str; 3]> {
    token.split('.').collect::<Vec<_>>().try_into().ok()
}

fn decode_json_object(segment: &str) -> Option<Map<String, Value>> {
    let bytes = URL_SAFE_NO_PAD.decode(segment).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn signed_token_verifies_with_the_same_secret() {
        // Arrange
        let token = sign_token(&json!({ "appId": "APP" }), "secret").unwrap();

        // Act / Assert
        assert_eq!(verify_signature(&token, "secret"), Ok(()));
    }

    #[test]
    fn decoded_claims_are_the_signed_payload() {
        // Arrange
        let token = sign_token(&json!({ "appId": "APP", "iat": 1 }), "secret").unwrap();

        // Act
        let claims = decode_claims(&token).unwrap();

        // Assert
        assert_eq!(Value::Object(claims), json!({ "appId": "APP", "iat": 1 }));
    }

    #[test]
    fn token_signed_with_another_secret_is_invalid() {
        // Arrange
        let token = sign_token(&json!({ "appId": "APP" }), "secret").unwrap();

        // Act / Assert
        assert_eq!(
            verify_signature(&token, "other"),
            Err(RejectReason::InvalidSignature)
        );
    }

    #[test]
    fn algorithm_other_than_hs256_is_invalid() {
        // Arrange
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(r#"{"appId":"APP"}"#);
        let token = format!("{header}.{payload}.");

        // Act / Assert
        assert_eq!(
            verify_signature(&token, "secret"),
            Err(RejectReason::InvalidSignature)
        );
    }

    #[test]
    fn token_without_three_segments_is_malformed() {
        // Act / Assert
        assert_eq!(decode_claims("not-a-jwt"), None);
        assert_eq!(
            verify_signature("a.b", "secret"),
            Err(RejectReason::MalformedToken)
        );
    }

    #[test]
    fn payload_that_is_not_a_json_object_is_malformed() {
        // Arrange
        let token = sign_token(&json!(["APP"]), "secret").unwrap();

        // Act / Assert
        assert_eq!(decode_claims(&token), None);
    }

    #[test]
    fn signature_that_is_not_base64url_is_malformed() {
        // Arrange
        let token = sign_token(&json!({ "appId": "APP" }), "secret").unwrap();
        let signing_input = token.rsplit_once('.').unwrap().0;
        let tampered = format!("{signing_input}.!!!");

        // Act / Assert
        assert_eq!(
            verify_signature(&tampered, "secret"),
            Err(RejectReason::MalformedToken)
        );
    }
}
