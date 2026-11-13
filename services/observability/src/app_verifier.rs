use std::{
    collections::HashMap,
    fmt,
    sync::Mutex,
    time::{Duration, SystemTime},
};

use auth_token::{ClaimPolicy, SignedToken};
use reqwest::{StatusCode, header::HeaderMap};
use serde_json::json;

const VERIFY_TIMEOUT: Duration = Duration::from_secs(3);
/// Verified tokens kept; past it, the cache starts over.
const MAX_CACHED_TOKENS: usize = 1024;

#[derive(Debug)]
pub enum VerifyError {
    Unauthorized(String),
    Unavailable(anyhow::Error),
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthorized(reason) => write!(f, "the token was rejected: {reason}"),
            Self::Unavailable(error) => write!(f, "the VTS could not verify the token: {error:#}"),
        }
    }
}

/// Verifies a client token through the VTS `POST /verify` and judges its
/// claims as the relay does. A token that passed is answered from memory until
/// it no longer passes, so a page polling every second asks the VTS once.
pub struct AppVerifier {
    http: reqwest::Client,
    verify_url: String,
    verified: Mutex<HashMap<String, SignedToken>>,
}

impl AppVerifier {
    pub fn new(verify_url: String) -> anyhow::Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(VERIFY_TIMEOUT).build()?,
            verify_url,
            verified: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) async fn verify(&self, token: &str) -> Result<SignedToken, VerifyError> {
        if let Some(cached) = self.cached(token, SystemTime::now()) {
            return Ok(cached);
        }
        let signed = self.verify_with_vts(token).await?;
        self.remember(token, &signed);
        Ok(signed)
    }

    fn cached(&self, token: &str, now: SystemTime) -> Option<SignedToken> {
        let mut verified = self
            .verified
            .lock()
            .expect("no panic while the lock is held");
        let signed = verified.get(token)?.clone();
        if signed.check(ClaimPolicy::default(), now).is_ok() {
            return Some(signed);
        }
        verified.remove(token);
        None
    }

    fn remember(&self, token: &str, signed: &SignedToken) {
        let mut verified = self
            .verified
            .lock()
            .expect("no panic while the lock is held");
        if verified.len() >= MAX_CACHED_TOKENS {
            verified.clear();
        }
        verified.insert(token.to_string(), signed.clone());
    }

    #[tracing::instrument(
        name = "observability.vts.verify",
        skip_all,
        fields(outcome = tracing::field::Empty)
    )]
    async fn verify_with_vts(&self, token: &str) -> Result<SignedToken, VerifyError> {
        let result = self.request_verification(token).await;
        tracing::Span::current().record("outcome", outcome(&result));
        result
    }

    async fn request_verification(&self, token: &str) -> Result<SignedToken, VerifyError> {
        let mut headers = HeaderMap::new();
        telemetry::inject_trace_context(&tracing::Span::current(), &mut headers);
        let response = self
            .http
            .post(&self.verify_url)
            .headers(headers)
            .body(json!({ "token": token }).to_string())
            .send()
            .await
            .map_err(|error| VerifyError::Unavailable(error.into()))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|error| VerifyError::Unavailable(error.into()))?;
        match status {
            StatusCode::OK => {
                let signed: SignedToken = serde_json::from_slice(&body)
                    .map_err(|error| VerifyError::Unavailable(error.into()))?;
                signed
                    .check(ClaimPolicy::default(), SystemTime::now())
                    .map_err(|error| VerifyError::Unauthorized(error.to_string()))?;
                Ok(signed)
            }
            StatusCode::UNAUTHORIZED => Err(VerifyError::Unauthorized(
                String::from_utf8_lossy(&body).into_owned(),
            )),
            status => Err(VerifyError::Unavailable(anyhow::anyhow!(
                "unexpected VTS status {status}"
            ))),
        }
    }
}

fn outcome(result: &Result<SignedToken, VerifyError>) -> &'static str {
    match result {
        Ok(_) => "verified",
        Err(VerifyError::Unauthorized(_)) => "unauthorized",
        Err(VerifyError::Unavailable(_)) => "unavailable",
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use hyper::StatusCode;

    use super::{AppVerifier, VerifyError};
    use crate::test_support::fake_http_server_answering;

    fn answer(exp: u64) -> String {
        format!(
            r#"{{"appId":"app-1","isRelay":false,"claims":{{"appId":"app-1","subscribe":"","iat":{},"exp":{exp}}}}}"#,
            exp - 60
        )
    }

    fn unix_now() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[tokio::test]
    async fn a_verified_token_is_answered_from_memory_until_it_expires() {
        // Arrange
        let exp = unix_now() + 30;
        let (vts_url, mut vts_requests) =
            fake_http_server_answering(StatusCode::OK, &answer(exp)).await;
        let verifier = AppVerifier::new(format!("{vts_url}/verify")).unwrap();
        let expired = SystemTime::UNIX_EPOCH + Duration::from_secs(exp + 61);

        // Act
        let first = verifier.verify("jwt").await.unwrap();
        let second = verifier.verify("jwt").await.unwrap();

        // Assert
        assert_eq!(
            (first.app_id.as_str(), second.app_id.as_str()),
            ("app-1", "app-1")
        );
        assert!(vts_requests.recv().await.is_some());
        assert!(vts_requests.try_recv().is_err());
        assert!(verifier.cached("jwt", expired).is_none());
    }

    #[tokio::test]
    async fn a_token_the_vts_signed_but_already_expired_is_unauthorized() {
        // Arrange
        let (vts_url, _vts_requests) =
            fake_http_server_answering(StatusCode::OK, &answer(1_000)).await;
        let verifier = AppVerifier::new(format!("{vts_url}/verify")).unwrap();

        // Act
        let result = verifier.verify("jwt").await;

        // Assert
        assert!(matches!(result, Err(VerifyError::Unauthorized(_))));
    }
}
