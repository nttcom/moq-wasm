use async_trait::async_trait;

use crate::modules::auth::verified_token::VerifiedToken;

#[derive(Debug, thiserror::Error)]
pub(crate) enum VerifyError {
    #[error("token rejected: {0}")]
    Unauthorized(String),
    #[error("token verification unavailable: {0}")]
    Unavailable(#[source] anyhow::Error),
}

#[async_trait]
pub(crate) trait TokenVerifier: Send + Sync {
    async fn verify(&self, token: &str) -> Result<VerifiedToken, VerifyError>;
}
