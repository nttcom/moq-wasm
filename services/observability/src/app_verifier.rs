use std::{collections::HashMap, sync::Mutex, time::SystemTime};

use auth_token::{ClaimPolicy, SignedToken, VtsClient, VtsVerifyError};

/// Verified tokens kept; past it, the cache starts over.
const MAX_CACHED_TOKENS: usize = 1024;

/// Verifies a client token through the VTS `POST /verify` and judges its
/// claims as the relay does. A token that passed is answered from memory until
/// it no longer passes, so a page polling every second asks the VTS once.
pub struct AppVerifier {
    vts: VtsClient,
    verified: Mutex<HashMap<String, SignedToken>>,
}

impl AppVerifier {
    pub fn new(verify_url: String) -> Self {
        Self {
            vts: VtsClient::new(verify_url),
            verified: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) async fn verify(&self, token: &str) -> Result<SignedToken, VtsVerifyError> {
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
    async fn verify_with_vts(&self, token: &str) -> Result<SignedToken, VtsVerifyError> {
        let result = self.request_verification(token).await;
        tracing::Span::current().record("outcome", outcome(&result));
        result
    }

    async fn request_verification(&self, token: &str) -> Result<SignedToken, VtsVerifyError> {
        let signed = self.vts.verify(token).await?;
        signed
            .check(ClaimPolicy::default(), SystemTime::now())
            .map_err(|error| VtsVerifyError::Unauthorized(error.to_string()))?;
        Ok(signed)
    }
}

fn outcome(result: &Result<SignedToken, VtsVerifyError>) -> &'static str {
    match result {
        Ok(_) => "verified",
        Err(VtsVerifyError::Unauthorized(_)) => "unauthorized",
        Err(_) => "unavailable",
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use auth_token::{VtsVerifyError, unix_now};
    use http_server::testing::StubServer;
    use hyper::StatusCode;

    use super::AppVerifier;

    fn answer(exp: u64) -> String {
        format!(
            r#"{{"appId":"app-1","isRelay":false,"claims":{{"appId":"app-1","subscribe":"","iat":{},"exp":{exp}}}}}"#,
            exp - 60
        )
    }

    #[tokio::test]
    async fn a_verified_token_is_answered_from_memory_until_it_expires() {
        // Arrange
        let exp = unix_now() + 30;
        let vts = StubServer::run(StatusCode::OK, answer(exp)).await;
        let verifier = AppVerifier::new(format!("{}/verify", vts.url()));
        let expired = SystemTime::UNIX_EPOCH + Duration::from_secs(exp + 61);

        // Act
        let first = verifier.verify("jwt").await.unwrap();
        let second = verifier.verify("jwt").await.unwrap();

        // Assert
        assert_eq!(
            (first.app_id.as_str(), second.app_id.as_str()),
            ("app-1", "app-1")
        );
        assert_eq!(vts.requests().len(), 1);
        assert!(verifier.cached("jwt", expired).is_none());
    }

    #[tokio::test]
    async fn a_token_the_vts_signed_but_already_expired_is_unauthorized() {
        // Arrange
        let vts = StubServer::run(StatusCode::OK, answer(1_000)).await;
        let verifier = AppVerifier::new(format!("{}/verify", vts.url()));

        // Act
        let result = verifier.verify("jwt").await;

        // Assert
        assert!(matches!(result, Err(VtsVerifyError::Unauthorized(_))));
    }
}
