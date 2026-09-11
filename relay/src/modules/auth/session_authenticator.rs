use std::sync::Arc;

use moqt::{TerminationErrorCode, wire::ClientSetup};

use crate::{
    AuthConfig,
    modules::{
        auth::{
            client_setup_token::{SetupTokenError, extract_token},
            token_verifier::{TokenVerifier, VerifyError},
            verified_token::VerifiedToken,
            vts_token_verifier::VtsTokenVerifier,
        },
        session_repository::SessionPeer,
    },
};

pub(crate) struct SessionAuthenticator {
    pub(crate) verifier: Arc<dyn TokenVerifier>,
}

#[derive(Debug)]
pub(crate) struct Rejected {
    pub(crate) code: TerminationErrorCode,
    pub(crate) reason: String,
}

impl SessionAuthenticator {
    pub(crate) fn from_config(auth: &AuthConfig) -> anyhow::Result<Self> {
        Ok(Self {
            verifier: Arc::new(VtsTokenVerifier::new(
                auth.verify_url.clone(),
                auth.claim_policy,
            )?),
        })
    }

    pub(crate) async fn authenticate(
        &self,
        client_setup: &ClientSetup,
        accepted_peer: &SessionPeer,
    ) -> Result<VerifiedToken, Rejected> {
        let relay_endpoint = matches!(accepted_peer, SessionPeer::Relay { .. });
        let token = match extract_token(client_setup) {
            Ok(token) => token,
            // A client that presents no token is accepted with the anonymous
            // scope (anon/**); the inter-relay endpoint still requires a token.
            Err(SetupTokenError::Missing) if !relay_endpoint => {
                return Ok(VerifiedToken::anonymous());
            }
            Err(error) => {
                return Err(Rejected {
                    code: TerminationErrorCode::Unauthorized,
                    reason: error.to_string(),
                });
            }
        };
        let verified = self
            .verifier
            .verify(&token)
            .await
            .map_err(|error| match error {
                VerifyError::Unauthorized(reason) => Rejected {
                    code: TerminationErrorCode::Unauthorized,
                    reason,
                },
                VerifyError::Unavailable(source) => {
                    tracing::error!(error = %source, "token verification unavailable");
                    Rejected {
                        code: TerminationErrorCode::InternalError,
                        reason: "token verification unavailable".to_string(),
                    }
                }
            })?;
        if verified.is_relay != relay_endpoint {
            let reason = if verified.is_relay {
                "relay token presented on the client endpoint"
            } else {
                "client token presented on the inter-relay endpoint"
            };
            return Err(Rejected {
                code: TerminationErrorCode::Unauthorized,
                reason: reason.to_string(),
            });
        }
        Ok(verified)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use moqt::{TerminationErrorCode, wire::AuthorizationToken};

    use super::SessionAuthenticator;
    use crate::modules::{
        auth::{
            test_support::{StubOutcome, StubVerifier, client_setup},
            verified_token::VerifiedToken,
        },
        session_repository::SessionPeer,
    };

    fn authenticator(outcome: StubOutcome) -> SessionAuthenticator {
        SessionAuthenticator {
            verifier: Arc::new(StubVerifier(outcome)),
        }
    }

    fn app_token() -> VerifiedToken {
        VerifiedToken {
            app_id: "APP".to_string(),
            publish: Some(vec![]),
            subscribe: None,
            is_relay: false,
            expires_at: None,
        }
    }

    fn relay_token() -> VerifiedToken {
        VerifiedToken {
            is_relay: true,
            ..app_token()
        }
    }

    fn setup_with_jwt() -> moqt::wire::ClientSetup {
        client_setup(vec![AuthorizationToken::use_value_utf8("jwt")])
    }

    #[tokio::test]
    async fn missing_token_on_client_endpoint_is_anonymous() {
        // Arrange
        let authenticator = authenticator(StubOutcome::Verified(app_token()));

        // Act
        let token = authenticator
            .authenticate(&client_setup(vec![]), &SessionPeer::Client)
            .await
            .unwrap();

        // Assert
        assert_eq!(token, VerifiedToken::anonymous());
    }

    #[tokio::test]
    async fn missing_token_on_inter_relay_endpoint_is_unauthorized() {
        // Arrange
        let authenticator = authenticator(StubOutcome::Verified(app_token()));

        // Act
        let rejected = authenticator
            .authenticate(
                &client_setup(vec![]),
                &SessionPeer::Relay { relay_id: None },
            )
            .await
            .unwrap_err();

        // Assert
        assert_eq!(rejected.code, TerminationErrorCode::Unauthorized);
    }

    #[tokio::test]
    async fn rejected_token_is_unauthorized_with_the_vts_reason() {
        // Arrange
        let authenticator = authenticator(StubOutcome::Unauthorized);

        // Act
        let rejected = authenticator
            .authenticate(&setup_with_jwt(), &SessionPeer::Client)
            .await
            .unwrap_err();

        // Assert
        assert_eq!(rejected.code, TerminationErrorCode::Unauthorized);
        assert_eq!(rejected.reason, "invalid_signature");
    }

    #[tokio::test]
    async fn unavailable_verifier_is_an_internal_error() {
        // Arrange
        let authenticator = authenticator(StubOutcome::Unavailable);

        // Act
        let rejected = authenticator
            .authenticate(&setup_with_jwt(), &SessionPeer::Client)
            .await
            .unwrap_err();

        // Assert
        assert_eq!(rejected.code, TerminationErrorCode::InternalError);
    }

    #[tokio::test]
    async fn relay_token_on_client_endpoint_is_unauthorized() {
        // Arrange
        let authenticator = authenticator(StubOutcome::Verified(relay_token()));

        // Act
        let rejected = authenticator
            .authenticate(&setup_with_jwt(), &SessionPeer::Client)
            .await
            .unwrap_err();

        // Assert
        assert_eq!(rejected.code, TerminationErrorCode::Unauthorized);
    }

    #[tokio::test]
    async fn client_token_on_inter_relay_endpoint_is_unauthorized() {
        // Arrange
        let authenticator = authenticator(StubOutcome::Verified(app_token()));

        // Act
        let rejected = authenticator
            .authenticate(&setup_with_jwt(), &SessionPeer::Relay { relay_id: None })
            .await
            .unwrap_err();

        // Assert
        assert_eq!(rejected.code, TerminationErrorCode::Unauthorized);
    }

    #[tokio::test]
    async fn matching_token_and_endpoint_returns_the_claims() {
        // Arrange
        let authenticator = authenticator(StubOutcome::Verified(app_token()));

        // Act
        let token = authenticator
            .authenticate(&setup_with_jwt(), &SessionPeer::Client)
            .await
            .unwrap();

        // Assert
        assert_eq!(token, app_token());
    }
}
