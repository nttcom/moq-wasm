use moqt::wire::AuthorizationToken;

use crate::modules::{
    auth::{
        token_parameter::{TokenParameterError, extract_token},
        token_verifier::{TokenVerifier, VerifyError},
        verified_token::VerifiedToken,
    },
    enums::SubscribeErrorCode,
};

#[derive(Debug)]
pub(crate) struct RefreshRejected {
    pub(crate) code: SubscribeErrorCode,
    pub(crate) reason: String,
}

fn rejected(code: SubscribeErrorCode, reason: impl Into<String>) -> RefreshRejected {
    RefreshRejected {
        code,
        reason: reason.into(),
    }
}

pub(crate) async fn refresh_token(
    verifier: &dyn TokenVerifier,
    current: Option<&VerifiedToken>,
    authorization_tokens: &[AuthorizationToken],
) -> Result<VerifiedToken, RefreshRejected> {
    let Some(current) = current else {
        return Err(rejected(
            SubscribeErrorCode::Unauthorized,
            "session has no verified token",
        ));
    };
    if current.is_relay {
        return Err(rejected(
            SubscribeErrorCode::NotSupported,
            "token refresh is not supported on inter-relay sessions",
        ));
    }
    let token = extract_token(authorization_tokens).map_err(|error| match error {
        TokenParameterError::Missing => rejected(
            SubscribeErrorCode::NotSupported,
            "TRACK_STATUS without an AUTHORIZATION TOKEN is not supported",
        ),
        malformed => rejected(
            SubscribeErrorCode::MalformedAuthToken,
            malformed.to_string(),
        ),
    })?;
    let verified = verifier.verify(&token).await.map_err(|error| match error {
        VerifyError::Unauthorized(reason) => rejected(SubscribeErrorCode::Unauthorized, reason),
        VerifyError::Unavailable(source) => {
            tracing::error!(error = %source, "token verification unavailable");
            rejected(
                SubscribeErrorCode::InternalError,
                "token verification unavailable",
            )
        }
    })?;
    if verified.is_relay {
        return Err(rejected(
            SubscribeErrorCode::Unauthorized,
            "relay token presented on a client session",
        ));
    }
    if verified.app_id != current.app_id {
        return Err(rejected(
            SubscribeErrorCode::Unauthorized,
            "token app_id differs from the session's",
        ));
    }
    Ok(verified)
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use moqt::wire::AuthorizationToken;

    use super::{RefreshRejected, refresh_token};
    use crate::modules::{
        auth::{
            test_support::{
                StubOutcome, StubVerifier, connect_client_with_token, spawn_relay_with_verifier,
            },
            verified_token::VerifiedToken,
        },
        enums::SubscribeErrorCode,
    };

    fn client_token(app_id: &str) -> VerifiedToken {
        VerifiedToken {
            app_id: app_id.to_string(),
            publish: Some(vec![]),
            subscribe: None,
            is_relay: false,
            expires_at: None,
        }
    }

    fn relay_token() -> VerifiedToken {
        VerifiedToken {
            is_relay: true,
            ..client_token("APP")
        }
    }

    fn jwt() -> Vec<AuthorizationToken> {
        vec![AuthorizationToken::use_value_utf8("jwt")]
    }

    async fn refresh(
        outcome: StubOutcome,
        current: Option<&VerifiedToken>,
        tokens: &[AuthorizationToken],
    ) -> Result<VerifiedToken, RefreshRejected> {
        refresh_token(&StubVerifier(outcome), current, tokens).await
    }

    fn code(result: Result<VerifiedToken, RefreshRejected>) -> SubscribeErrorCode {
        result.unwrap_err().code
    }

    #[tokio::test]
    async fn verified_token_of_the_same_app_is_adopted() {
        // Arrange
        let current = client_token("APP");
        let renewed = VerifiedToken {
            subscribe: Some(vec!["site1".to_string()]),
            ..client_token("APP")
        };

        // Act
        let result = refresh(
            StubOutcome::Verified(renewed.clone()),
            Some(&current),
            &jwt(),
        )
        .await;

        // Assert
        assert_eq!(result.unwrap(), renewed);
    }

    #[tokio::test]
    async fn missing_token_is_not_supported() {
        // Arrange
        let current = client_token("APP");

        // Act
        let result = refresh(StubOutcome::Verified(current.clone()), Some(&current), &[]).await;

        // Assert
        assert_eq!(code(result), SubscribeErrorCode::NotSupported);
    }

    #[tokio::test]
    async fn register_alias_type_is_malformed() {
        // Arrange
        let current = client_token("APP");
        let tokens = [AuthorizationToken::Register {
            token_alias: 1,
            token_type: 0,
            token_value: Bytes::from_static(b"jwt"),
        }];

        // Act
        let result = refresh(
            StubOutcome::Verified(current.clone()),
            Some(&current),
            &tokens,
        )
        .await;

        // Assert
        assert_eq!(code(result), SubscribeErrorCode::MalformedAuthToken);
    }

    #[tokio::test]
    async fn rejected_token_is_unauthorized() {
        // Arrange
        let current = client_token("APP");

        // Act
        let result = refresh(StubOutcome::Unauthorized, Some(&current), &jwt()).await;

        // Assert
        assert_eq!(code(result), SubscribeErrorCode::Unauthorized);
    }

    #[tokio::test]
    async fn unavailable_verifier_is_an_internal_error() {
        // Arrange
        let current = client_token("APP");

        // Act
        let result = refresh(StubOutcome::Unavailable, Some(&current), &jwt()).await;

        // Assert
        assert_eq!(code(result), SubscribeErrorCode::InternalError);
    }

    #[tokio::test]
    async fn relay_token_is_unauthorized() {
        // Arrange
        let current = client_token("APP");

        // Act
        let result = refresh(StubOutcome::Verified(relay_token()), Some(&current), &jwt()).await;

        // Assert
        assert_eq!(code(result), SubscribeErrorCode::Unauthorized);
    }

    #[tokio::test]
    async fn token_of_another_app_is_unauthorized() {
        // Arrange
        let current = client_token("APP");

        // Act
        let result = refresh(
            StubOutcome::Verified(client_token("OTHER")),
            Some(&current),
            &jwt(),
        )
        .await;

        // Assert
        assert_eq!(code(result), SubscribeErrorCode::Unauthorized);
    }

    #[tokio::test]
    async fn inter_relay_session_is_not_supported() {
        // Arrange
        let current = relay_token();

        // Act
        let result = refresh(
            StubOutcome::Verified(current.clone()),
            Some(&current),
            &jwt(),
        )
        .await;

        // Assert
        assert_eq!(code(result), SubscribeErrorCode::NotSupported);
    }

    #[tokio::test]
    async fn session_without_a_token_is_unauthorized() {
        // Act
        let result = refresh(StubOutcome::Verified(client_token("APP")), None, &jwt()).await;

        // Assert
        assert_eq!(code(result), SubscribeErrorCode::Unauthorized);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn track_status_carrying_a_token_is_answered_with_ok() {
        // Arrange
        let relay = spawn_relay_with_verifier(client_token("APP")).await;
        let client = connect_client_with_token(relay.port, "jwt").await;

        // Act
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.subscriber().track_status(
                "APP".to_string(),
                "update_auth_token".to_string(),
                vec![AuthorizationToken::use_value_utf8("renewed-jwt")],
            ),
        )
        .await
        .unwrap();

        // Assert
        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn track_status_without_a_token_is_answered_with_not_supported() {
        // Arrange
        let relay = spawn_relay_with_verifier(client_token("APP")).await;
        let client = connect_client_with_token(relay.port, "jwt").await;

        // Act
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client
                .subscriber()
                .track_status("APP".to_string(), "cam1".to_string(), vec![]),
        )
        .await
        .unwrap();

        // Assert
        let error = result.unwrap_err();
        let request_error = error
            .downcast_ref::<moqt::wire::RequestError>()
            .unwrap_or_else(|| panic!("expected a request error, got {error:?}"));
        assert_eq!(
            request_error.error_code,
            SubscribeErrorCode::NotSupported as u64
        );
    }
}
