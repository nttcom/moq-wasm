use std::time::SystemTime;

use async_trait::async_trait;
use auth_token::{ClaimPolicy, VtsClient, VtsVerifyError};

use crate::modules::auth::{
    token_claims::build_verified_token,
    token_verifier::{TokenVerifier, VerifyError},
    verified_token::VerifiedToken,
};

pub(crate) struct VtsTokenVerifier {
    client: VtsClient,
    claim_policy: ClaimPolicy,
}

impl VtsTokenVerifier {
    pub(crate) fn new(verify_url: String, claim_policy: ClaimPolicy) -> anyhow::Result<Self> {
        Ok(Self {
            client: VtsClient::new(verify_url),
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
        let signed = self
            .client
            .verify(token)
            .await
            .map_err(|error| match error {
                VtsVerifyError::Unauthorized(reason) => VerifyError::Unauthorized(reason),
                other => VerifyError::Unavailable(other.into()),
            })?;
        build_verified_token(signed, self.claim_policy, SystemTime::now())
            .map_err(|reason| VerifyError::Unauthorized(reason.to_string()))
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
    use crate::modules::auth::{token_verifier::VerifyError, verified_token::VerifiedToken};

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
}
