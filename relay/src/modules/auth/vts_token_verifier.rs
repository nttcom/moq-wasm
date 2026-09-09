use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::modules::auth::{
    token_verifier::{TokenVerifier, VerifyError},
    verified_token::{VerifiedToken, parse_namespace_path},
};

const VERIFY_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) struct VtsTokenVerifier {
    client: reqwest::Client,
    verify_url: String,
}

#[derive(Serialize)]
struct VerifyRequest<'a> {
    token: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VerifyResponse {
    app_id: String,
    publish: Option<String>,
    subscribe: Option<String>,
    is_relay: bool,
    exp: u64,
}

#[derive(Deserialize)]
struct VerifyErrorResponse {
    error: String,
}

impl From<VerifyResponse> for VerifiedToken {
    fn from(response: VerifyResponse) -> Self {
        Self {
            app_id: response.app_id,
            publish: response.publish.as_deref().map(parse_namespace_path),
            subscribe: response.subscribe.as_deref().map(parse_namespace_path),
            is_relay: response.is_relay,
            expires_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(response.exp)),
        }
    }
}

impl VtsTokenVerifier {
    pub(crate) fn new(verify_url: String) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder().timeout(VERIFY_TIMEOUT).build()?;
        Ok(Self { client, verify_url })
    }
}

#[async_trait]
impl TokenVerifier for VtsTokenVerifier {
    async fn verify(&self, token: &str) -> Result<VerifiedToken, VerifyError> {
        let response = self
            .client
            .post(&self.verify_url)
            .json(&VerifyRequest { token })
            .send()
            .await
            .map_err(|error| VerifyError::Unavailable(error.into()))?;
        match response.status() {
            StatusCode::OK => response
                .json::<VerifyResponse>()
                .await
                .map(VerifiedToken::from)
                .map_err(|error| VerifyError::Unavailable(error.into())),
            StatusCode::UNAUTHORIZED => {
                let reason = response
                    .json::<VerifyErrorResponse>()
                    .await
                    .map(|body| body.error)
                    .unwrap_or_else(|_| "unauthorized".to_string());
                Err(VerifyError::Unauthorized(reason))
            }
            status => Err(VerifyError::Unavailable(anyhow::anyhow!(
                "unexpected VTS status {status}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::VerifyResponse;
    use crate::modules::auth::verified_token::VerifiedToken;

    #[test]
    fn response_claims_are_converted_to_paths_and_expiry() {
        // Arrange
        let response: VerifyResponse = serde_json::from_str(
            r#"{"appId":"APP","publish":"site1/cam1","subscribe":"","isRelay":false,"exp":1759678000}"#,
        )
        .unwrap();

        // Act
        let token = VerifiedToken::from(response);

        // Assert
        assert_eq!(
            token,
            VerifiedToken {
                app_id: "APP".to_string(),
                publish: Some(vec!["site1".to_string(), "cam1".to_string()]),
                subscribe: Some(vec![]),
                is_relay: false,
                expires_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1759678000)),
            }
        );
    }

    #[test]
    fn absent_claims_stay_absent() {
        // Arrange
        let response: VerifyResponse = serde_json::from_str(
            r#"{"appId":"APP","publish":null,"subscribe":"site1","isRelay":true,"exp":1}"#,
        )
        .unwrap();

        // Act
        let token = VerifiedToken::from(response);

        // Assert
        assert_eq!(token.publish, None);
        assert_eq!(token.subscribe, Some(vec!["site1".to_string()]));
        assert!(token.is_relay);
    }
}
