use std::{
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::{StatusCode, header::HeaderMap};
use serde::Deserialize;
use serde_json::json;

const VERIFY_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub(super) enum VerifyError {
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

/// Reads a client token's app through the VTS `POST /verify`, as the relay does.
pub(super) struct AppVerifier {
    http: reqwest::Client,
    verify_url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyResponse {
    app_id: String,
    claims: VerifiedClaims,
}

#[derive(Deserialize)]
struct VerifiedClaims {
    exp: Option<u64>,
}

impl AppVerifier {
    pub(super) fn new(verify_url: String) -> anyhow::Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(VERIFY_TIMEOUT).build()?,
            verify_url,
        })
    }

    #[tracing::instrument(
        name = "observability.vts.verify",
        skip_all,
        fields(outcome = tracing::field::Empty)
    )]
    pub(super) async fn app_id(&self, token: &str) -> Result<String, VerifyError> {
        let result = self.request_verification(token).await;
        tracing::Span::current().record("outcome", outcome(&result));
        result
    }

    async fn request_verification(&self, token: &str) -> Result<String, VerifyError> {
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
                let verified: VerifyResponse = serde_json::from_slice(&body)
                    .map_err(|error| VerifyError::Unavailable(error.into()))?;
                check_expiry(verified.claims.exp, SystemTime::now())?;
                Ok(verified.app_id)
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

/// The VTS checks only the signature; the expiry is the caller's.
fn check_expiry(exp: Option<u64>, now: SystemTime) -> Result<(), VerifyError> {
    let exp = exp.ok_or_else(|| VerifyError::Unauthorized("token has no exp".to_string()))?;
    if UNIX_EPOCH + Duration::from_secs(exp) <= now {
        return Err(VerifyError::Unauthorized("expired".to_string()));
    }
    Ok(())
}

fn outcome(result: &Result<String, VerifyError>) -> &'static str {
    match result {
        Ok(_) => "verified",
        Err(VerifyError::Unauthorized(_)) => "unauthorized",
        Err(VerifyError::Unavailable(_)) => "unavailable",
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::{VerifyError, check_expiry};

    #[test]
    fn a_token_is_valid_until_its_exp() {
        // Arrange
        let exp = 1_000;
        let before = UNIX_EPOCH + Duration::from_secs(999);
        let at = UNIX_EPOCH + Duration::from_secs(1_000);

        // Act / Assert
        assert!(check_expiry(Some(exp), before).is_ok());
        assert!(matches!(
            check_expiry(Some(exp), at),
            Err(VerifyError::Unauthorized(_))
        ));
        assert!(matches!(
            check_expiry(None, before),
            Err(VerifyError::Unauthorized(_))
        ));
    }
}
