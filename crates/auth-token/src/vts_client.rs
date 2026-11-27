use std::time::{Duration, SystemTime};

use reqwest::{StatusCode, header::HeaderMap};
use serde::{Deserialize, Serialize};

use crate::{Access, ClaimError, ClaimPolicy, SignedToken};

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

/// Why `VtsClient::authorize` refused a token.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AuthorizeError {
    #[error("no token")]
    MissingToken,
    #[error("token rejected: {0}")]
    Unauthorized(String),
    #[error("VTS unavailable: {0:#}")]
    Unavailable(#[source] anyhow::Error),
    /// The lifetime or a namespace claim fails `SignedToken::check`.
    #[error("invalid claims: {0}")]
    InvalidClaims(#[source] ClaimError),
    /// A relay token, or a claim for the access that does not cover the
    /// namespace prefix.
    #[error("forbidden")]
    Forbidden,
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

    /// Verifies `token` and checks that the claim for `access` of a non-relay
    /// app covers `namespace_prefix` under `policy`.
    ///
    /// Fails with `MissingToken` when `token` is `None` (before calling the
    /// VTS), `Unauthorized` / `Unavailable` as `verify` does, `InvalidClaims`
    /// when `SignedToken::check` fails, and `Forbidden` for a relay token or
    /// a claim that does not cover the prefix.
    pub async fn authorize(
        &self,
        token: Option<&str>,
        access: Access,
        namespace_prefix: &str,
        policy: ClaimPolicy,
    ) -> Result<SignedToken, AuthorizeError> {
        let token = token.ok_or(AuthorizeError::MissingToken)?;
        let verified = self.verify(token).await.map_err(|error| match error {
            VtsVerifyError::Unauthorized(reason) => AuthorizeError::Unauthorized(reason),
            VtsVerifyError::Unavailable(error) => AuthorizeError::Unavailable(error),
        })?;
        check_access(
            &verified,
            access,
            namespace_prefix,
            policy,
            SystemTime::now(),
        )?;
        Ok(verified)
    }
}

fn check_access(
    verified: &SignedToken,
    access: Access,
    namespace_prefix: &str,
    policy: ClaimPolicy,
    now: SystemTime,
) -> Result<(), AuthorizeError> {
    verified
        .check(policy, now)
        .map_err(AuthorizeError::InvalidClaims)?;
    if verified.is_relay || !verified.covers(access, namespace_prefix) {
        return Err(AuthorizeError::Forbidden);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use http_server::testing::StubServer;
    use hyper::StatusCode;
    use serde_json::json;

    use super::*;

    const POLICY: ClaimPolicy = ClaimPolicy {
        max_token_ttl: Duration::from_secs(5 * 60),
        clock_leeway: Duration::from_secs(60),
    };

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn verified(is_relay: bool, claim: &str, value: Option<&str>, exp: u64) -> SignedToken {
        let mut claims = json!({ "appId": "APP", "iat": 100, "exp": exp });
        claims[claim] = json!(value);
        serde_json::from_value(json!({ "appId": "APP", "isRelay": is_relay, "claims": claims }))
            .unwrap()
    }

    #[test]
    fn only_a_short_lived_non_relay_claim_covering_the_prefix_passes() {
        for (access, claim) in [(Access::Bots, "bots"), (Access::Calls, "calls")] {
            // Act
            let check = |is_relay, value, exp, prefix| {
                check_access(
                    &verified(is_relay, claim, value, exp),
                    access,
                    prefix,
                    POLICY,
                    at(110),
                )
            };

            // Assert
            assert!(check(false, Some("room"), 400, "APP/room").is_ok());
            assert!(matches!(
                check(false, Some("room"), 400, "APP/room2"),
                Err(AuthorizeError::Forbidden)
            ));
            assert!(matches!(
                check(false, None, 400, "APP/room"),
                Err(AuthorizeError::Forbidden)
            ));
            assert!(matches!(
                check(true, Some(""), 400, "APP/room"),
                Err(AuthorizeError::Forbidden)
            ));
            assert!(matches!(
                check(false, Some("room"), 100 + 3600, "APP/room"),
                Err(AuthorizeError::InvalidClaims(ClaimError::TtlTooLong))
            ));
        }
    }

    #[tokio::test]
    async fn authorize_without_a_token_fails_before_calling_the_vts() {
        // Arrange
        let server = StubServer::run(StatusCode::OK, "{}").await;
        let client = VtsClient::new(format!("{}/verify", server.url()));

        // Act
        let error = client
            .authorize(None, Access::Bots, "APP/room", POLICY)
            .await
            .unwrap_err();

        // Assert
        assert!(matches!(error, AuthorizeError::MissingToken));
        assert!(server.requests().is_empty());
    }

    #[tokio::test]
    async fn authorize_returns_the_verified_token_or_the_vts_failure() {
        // Arrange
        let now = crate::unix_now();
        let body = json!({
            "appId": "APP",
            "isRelay": false,
            "claims": { "appId": "APP", "calls": "room", "iat": now, "exp": now + 60 },
        })
        .to_string();
        let ok = StubServer::run(StatusCode::OK, body).await;
        let rejecting = StubServer::run(StatusCode::UNAUTHORIZED, r#"{"error":"bad"}"#).await;
        let failing = StubServer::run(StatusCode::INTERNAL_SERVER_ERROR, "{}").await;
        let authorize = |server: &StubServer| {
            let client = VtsClient::new(format!("{}/verify", server.url()));
            async move {
                client
                    .authorize(Some("jwt"), Access::Calls, "APP/room/phone", POLICY)
                    .await
            }
        };

        // Act
        let verified = authorize(&ok).await;
        let unauthorized = authorize(&rejecting).await;
        let unavailable = authorize(&failing).await;

        // Assert
        assert_eq!(verified.unwrap().app_id, "APP");
        assert!(
            matches!(unauthorized, Err(AuthorizeError::Unauthorized(ref reason)) if reason == "bad")
        );
        assert!(matches!(unavailable, Err(AuthorizeError::Unavailable(_))));
    }

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
