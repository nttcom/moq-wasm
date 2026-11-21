use std::time::Duration;

use reqwest::{StatusCode, header::HeaderMap};
use serde::{Deserialize, Serialize};

use crate::SignedToken;

/// The VTS answers a signature check at once; a request that takes this long
/// is stuck.
const VERIFY_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VtsVerifyError {
    #[error("token rejected: {0}")]
    Unauthorized(String),
    #[error("VTS unavailable: {0:#}")]
    Unavailable(#[source] anyhow::Error),
}

#[derive(Serialize)]
struct VerifyRequest<'a> {
    token: &'a str,
}

#[derive(Deserialize)]
struct VerifyErrorResponse {
    error: String,
}

/// Verifies a client token against the VTS `POST /verify` endpoint.
///
/// The VTS checks only the signature; callers must apply `SignedToken::check`
/// with their own `ClaimPolicy` and authorization rules.
pub struct VtsClient {
    http: reqwest::Client,
    verify_url: String,
}

impl VtsClient {
    pub fn new(verify_url: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            verify_url,
        }
    }

    pub async fn verify(&self, token: &str) -> Result<SignedToken, VtsVerifyError> {
        let mut headers = HeaderMap::new();
        telemetry_otlp::inject_trace_context(&tracing::Span::current(), &mut headers);
        let response = self
            .http
            .post(&self.verify_url)
            .timeout(VERIFY_TIMEOUT)
            .headers(headers)
            .json(&VerifyRequest { token })
            .send()
            .await
            .map_err(|error| VtsVerifyError::Unavailable(error.into()))?;
        match response.status() {
            StatusCode::OK => response
                .json::<SignedToken>()
                .await
                .map_err(|error| VtsVerifyError::Unavailable(error.into())),
            StatusCode::UNAUTHORIZED => {
                let reason = response
                    .json::<VerifyErrorResponse>()
                    .await
                    .map(|body| body.error)
                    .unwrap_or_else(|_| "unauthorized".to_string());
                Err(VtsVerifyError::Unauthorized(reason))
            }
            status => Err(VtsVerifyError::Unavailable(anyhow::anyhow!(
                "unexpected VTS status {status}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use http_server::testing::StubServer;
    use hyper::StatusCode;

    use super::*;

    #[tokio::test]
    async fn verifies_a_signed_token_from_a_200_response() {
        // Arrange
        let body = r#"{"appId":"app-1","isRelay":false,"claims":{"appId":"app-1","subscribe":"","iat":100,"exp":200}}"#;
        let server = StubServer::run(StatusCode::OK, body).await;
        let client = VtsClient::new(format!("{}/verify", server.url()));

        // Act
        let signed = client.verify("jwt-token").await.unwrap();

        // Assert
        assert_eq!(signed.app_id, "app-1");
        assert!(!signed.is_relay);
        assert_eq!(signed.claims.subscribe.as_deref(), Some(""));
    }

    #[tokio::test]
    async fn maps_401_responses_to_unauthorized_with_reason_or_fallback() {
        // Arrange
        let with_reason =
            StubServer::run(StatusCode::UNAUTHORIZED, r#"{"error":"invalid signature"}"#).await;
        let without_json = StubServer::run(StatusCode::UNAUTHORIZED, "denied").await;
        let client_reason = VtsClient::new(format!("{}/verify", with_reason.url()));
        let client_fallback = VtsClient::new(format!("{}/verify", without_json.url()));

        // Act
        let err_reason = client_reason.verify("bad").await.unwrap_err();
        let err_fallback = client_fallback.verify("bad").await.unwrap_err();

        // Assert
        assert!(
            matches!(err_reason, VtsVerifyError::Unauthorized(ref reason) if reason == "invalid signature")
        );
        assert!(
            matches!(err_fallback, VtsVerifyError::Unauthorized(ref reason) if reason == "unauthorized")
        );
    }

    #[tokio::test]
    async fn maps_non_401_errors_and_unreachable_vts_to_unavailable() {
        // Arrange
        let server_error = StubServer::run(StatusCode::INTERNAL_SERVER_ERROR, "{}").await;
        let closed_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unreachable_url = format!("http://{}/verify", closed_listener.local_addr().unwrap());
        drop(closed_listener);
        let client_500 = VtsClient::new(format!("{}/verify", server_error.url()));
        let client_unreachable = VtsClient::new(unreachable_url);

        // Act
        let err_500 = client_500.verify("jwt").await.unwrap_err();
        let err_unreachable = client_unreachable.verify("jwt").await.unwrap_err();

        // Assert
        assert!(matches!(err_500, VtsVerifyError::Unavailable(_)));
        assert!(matches!(err_unreachable, VtsVerifyError::Unavailable(_)));
    }
}
