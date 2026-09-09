use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::modules::auth::{
    token_claims::{ClaimPolicy, SignedToken, TokenClaims, build_verified_token},
    token_verifier::{TokenVerifier, VerifyError},
    verified_token::VerifiedToken,
};

const VERIFY_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) struct VtsTokenVerifier {
    client: reqwest::Client,
    verify_url: String,
    claim_policy: ClaimPolicy,
}

#[derive(Serialize)]
struct VerifyRequest<'a> {
    token: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyResponse {
    app_id: String,
    is_relay: bool,
    claims: TokenClaims,
}

#[derive(Deserialize)]
struct VerifyErrorResponse {
    error: String,
}

impl From<VerifyResponse> for SignedToken {
    fn from(response: VerifyResponse) -> Self {
        Self {
            app_id: response.app_id,
            is_relay: response.is_relay,
            claims: response.claims,
        }
    }
}

impl VtsTokenVerifier {
    pub(crate) fn new(verify_url: String, claim_policy: ClaimPolicy) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder().timeout(VERIFY_TIMEOUT).build()?;
        Ok(Self {
            client,
            verify_url,
            claim_policy,
        })
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
            StatusCode::OK => {
                let signed: SignedToken = response
                    .json::<VerifyResponse>()
                    .await
                    .map_err(|error| VerifyError::Unavailable(error.into()))?
                    .into();
                build_verified_token(signed, self.claim_policy, SystemTime::now())
                    .map_err(|reason| VerifyError::Unauthorized(reason.to_string()))
            }
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
    use super::VerifyResponse;

    #[test]
    fn response_carries_the_raw_claims_and_the_relay_flag() {
        // Act
        let response: VerifyResponse = serde_json::from_str(
            r#"{"appId":"APP","isRelay":true,"claims":{"appId":"APP","publish":"","iat":1,"exp":2,"custom":"ignored"}}"#,
        )
        .unwrap();

        // Assert
        assert_eq!(response.app_id, "APP");
        assert!(response.is_relay);
        assert_eq!(response.claims.publish.as_deref(), Some(""));
        assert_eq!(response.claims.subscribe, None);
        assert_eq!(response.claims.iat, Some(1));
        assert_eq!(response.claims.exp, Some(2));
    }
}
