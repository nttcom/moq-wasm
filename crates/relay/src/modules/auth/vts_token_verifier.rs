use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use reqwest::{StatusCode, header::HeaderMap};
use serde::{Deserialize, Serialize};

use crate::modules::auth::{
    token_claims::{ClaimPolicy, SignedToken, build_verified_token},
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

#[derive(Deserialize)]
struct VerifyErrorResponse {
    error: String,
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
    #[tracing::instrument(
        name = "relay.vts.verify",
        skip_all,
        fields(outcome = tracing::field::Empty)
    )]
    async fn verify(&self, token: &str) -> Result<VerifiedToken, VerifyError> {
        let result = self.request_verification(token).await;
        tracing::Span::current().record("outcome", outcome(&result));
        result
    }
}

impl VtsTokenVerifier {
    async fn request_verification(&self, token: &str) -> Result<VerifiedToken, VerifyError> {
        let mut headers = HeaderMap::new();
        telemetry::inject_trace_context(&tracing::Span::current(), &mut headers);
        let response = self
            .client
            .post(&self.verify_url)
            .headers(headers)
            .json(&VerifyRequest { token })
            .send()
            .await
            .map_err(|error| VerifyError::Unavailable(error.into()))?;
        match response.status() {
            StatusCode::OK => {
                let signed = response
                    .json::<SignedToken>()
                    .await
                    .map_err(|error| VerifyError::Unavailable(error.into()))?;
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

fn outcome(result: &Result<VerifiedToken, VerifyError>) -> &'static str {
    match result {
        Ok(_) => "verified",
        Err(VerifyError::Unauthorized(_)) => "unauthorized",
        Err(VerifyError::Unavailable(_)) => "unavailable",
    }
}

#[cfg(test)]
mod tests {
    use super::outcome;
    use crate::modules::auth::{
        token_claims::SignedToken, token_verifier::VerifyError, verified_token::VerifiedToken,
    };

    #[test]
    fn outcome_names_each_verification_result() {
        // Arrange
        let verified = Ok(VerifiedToken::full_access());
        let rejected = Err(VerifyError::Unauthorized("token expired".to_string()));
        let unreachable = Err(VerifyError::Unavailable(anyhow::anyhow!(
            "connection refused"
        )));

        // Act / Assert
        assert_eq!(outcome(&verified), "verified");
        assert_eq!(outcome(&rejected), "unauthorized");
        assert_eq!(outcome(&unreachable), "unavailable");
    }

    #[test]
    fn response_carries_the_raw_claims_and_the_relay_flag() {
        // Act
        let response: SignedToken = serde_json::from_str(
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
