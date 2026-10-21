use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::hmac;
use serde::Serialize;
use serde_json::{Map, Value};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum SignatureError {
    #[error("token is not a compact JWS")]
    MalformedToken,
    #[error("token is not HS256-signed with the secret")]
    InvalidSignature,
}

pub fn sign_token(claims: &impl Serialize, secret: &str) -> anyhow::Result<String> {
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

pub fn decode_unverified_payload(token: &str) -> Option<Map<String, Value>> {
    let [_, payload, _] = split_compact(token)?;
    decode_json_object(payload)
}

/// Fails with `MalformedToken` when the token, its header, its `alg` or its signature cannot be read,
/// and with `InvalidSignature` when `alg` is not `HS256` or the HMAC does not match.
pub fn verify_signature(token: &str, secret: &str) -> Result<(), SignatureError> {
    let [header_segment, payload_segment, signature_segment] =
        split_compact(token).ok_or(SignatureError::MalformedToken)?;
    let header = decode_json_object(header_segment).ok_or(SignatureError::MalformedToken)?;
    let alg = header
        .get("alg")
        .and_then(Value::as_str)
        .ok_or(SignatureError::MalformedToken)?;
    if alg != "HS256" {
        return Err(SignatureError::InvalidSignature);
    }
    let signature = URL_SAFE_NO_PAD
        .decode(signature_segment)
        .map_err(|_| SignatureError::MalformedToken)?;
    let signing_input = &token[..header_segment.len() + 1 + payload_segment.len()];
    hmac::verify(&hmac_key(secret), signing_input.as_bytes(), &signature)
        .map_err(|_| SignatureError::InvalidSignature)
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
    fn decoded_unverified_payload_is_the_signed_payload() {
        // Arrange
        let token = sign_token(&json!({ "appId": "APP", "iat": 1 }), "secret").unwrap();

        // Act
        let payload = decode_unverified_payload(&token).unwrap();

        // Assert
        assert_eq!(Value::Object(payload), json!({ "appId": "APP", "iat": 1 }));
    }

    #[test]
    fn token_signed_with_another_secret_is_invalid() {
        // Arrange
        let token = sign_token(&json!({ "appId": "APP" }), "secret").unwrap();

        // Act / Assert
        assert_eq!(
            verify_signature(&token, "other"),
            Err(SignatureError::InvalidSignature)
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
            Err(SignatureError::InvalidSignature)
        );
    }

    #[test]
    fn token_without_three_segments_is_malformed() {
        // Act / Assert
        assert_eq!(decode_unverified_payload("not-a-jwt"), None);
        assert_eq!(
            verify_signature("a.b", "secret"),
            Err(SignatureError::MalformedToken)
        );
    }

    #[test]
    fn payload_that_is_not_a_json_object_is_malformed() {
        // Arrange
        let token = sign_token(&json!(["APP"]), "secret").unwrap();

        // Act / Assert
        assert_eq!(decode_unverified_payload(&token), None);
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
            Err(SignatureError::MalformedToken)
        );
    }
}
