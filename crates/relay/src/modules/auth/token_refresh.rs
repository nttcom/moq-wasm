use moqt::RequestErrorCode;
use moqt::wire::AuthorizationToken;

use crate::modules::auth::{
    token_parameter::{TokenParameterError, extract_token},
    token_verifier::{TokenVerifier, VerifyError},
    verified_token::VerifiedToken,
};

#[derive(Debug)]
pub(crate) struct RefreshRejected {
    pub(crate) code: RequestErrorCode,
    pub(crate) reason: String,
}

fn rejected(code: RequestErrorCode, reason: impl Into<String>) -> RefreshRejected {
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
            RequestErrorCode::Unauthorized,
            "session has no verified token",
        ));
    };
    if current.is_relay {
        return Err(rejected(
            RequestErrorCode::NotSupported,
            "token refresh is not supported on inter-relay sessions",
        ));
    }
    let token = extract_token(authorization_tokens).map_err(|error| match error {
        TokenParameterError::Missing => rejected(
            RequestErrorCode::NotSupported,
            "TRACK_STATUS without an AUTHORIZATION TOKEN is not supported",
        ),
        malformed => rejected(RequestErrorCode::MalformedAuthToken, malformed.to_string()),
    })?;
    let verified = verifier.verify(&token).await.map_err(|error| match error {
        VerifyError::Unauthorized(reason) => rejected(RequestErrorCode::Unauthorized, reason),
        VerifyError::Unavailable(source) => {
            tracing::error!(error = %source, "token verification unavailable");
            rejected(
                RequestErrorCode::InternalError,
                "token verification unavailable",
            )
        }
    })?;
    if verified.is_relay {
        return Err(rejected(
            RequestErrorCode::Unauthorized,
            "relay token presented on a client session",
        ));
    }
    if verified.app_id != current.app_id {
        return Err(rejected(
            RequestErrorCode::Unauthorized,
            "token app_id differs from the session's",
        ));
    }
    Ok(verified)
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use moqt::RequestErrorCode;
    use moqt::wire::AuthorizationToken;

    use super::{RefreshRejected, refresh_token};
    use crate::modules::auth::{
        test_support::{
            StubOutcome, StubVerifier, app_token, relay_token, spawn_relay_and_connect_client,
        },
        verified_token::VerifiedToken,
    };

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

    fn code(result: Result<VerifiedToken, RefreshRejected>) -> RequestErrorCode {
        result.unwrap_err().code
    }

    #[tokio::test]
    async fn verified_token_of_the_same_app_is_adopted() {
        // Arrange
        let current = app_token(Some(""), None);
        let renewed = VerifiedToken {
            subscribe: Some(vec!["site1".to_string()]),
            ..app_token(Some(""), None)
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
        let current = app_token(Some(""), None);

        // Act
        let result = refresh(StubOutcome::Verified(current.clone()), Some(&current), &[]).await;

        // Assert
        assert_eq!(code(result), RequestErrorCode::NotSupported);
    }

    #[tokio::test]
    async fn register_alias_type_is_malformed() {
        // Arrange
        let current = app_token(Some(""), None);
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
        assert_eq!(code(result), RequestErrorCode::MalformedAuthToken);
    }

    #[tokio::test]
    async fn rejected_token_is_unauthorized() {
        // Arrange
        let current = app_token(Some(""), None);

        // Act
        let result = refresh(StubOutcome::Unauthorized, Some(&current), &jwt()).await;

        // Assert
        assert_eq!(code(result), RequestErrorCode::Unauthorized);
    }

    #[tokio::test]
    async fn unavailable_verifier_is_an_internal_error() {
        // Arrange
        let current = app_token(Some(""), None);

        // Act
        let result = refresh(StubOutcome::Unavailable, Some(&current), &jwt()).await;

        // Assert
        assert_eq!(code(result), RequestErrorCode::InternalError);
    }

    #[tokio::test]
    async fn relay_token_is_unauthorized() {
        // Arrange
        let current = app_token(Some(""), None);

        // Act
        let result = refresh(StubOutcome::Verified(relay_token()), Some(&current), &jwt()).await;

        // Assert
        assert_eq!(code(result), RequestErrorCode::Unauthorized);
    }

    #[tokio::test]
    async fn token_of_another_app_is_unauthorized() {
        // Arrange
        let current = app_token(Some(""), None);

        // Act
        let result = refresh(
            StubOutcome::Verified(VerifiedToken {
                app_id: "OTHER".to_string(),
                ..app_token(Some(""), None)
            }),
            Some(&current),
            &jwt(),
        )
        .await;

        // Assert
        assert_eq!(code(result), RequestErrorCode::Unauthorized);
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
        assert_eq!(code(result), RequestErrorCode::NotSupported);
    }

    #[tokio::test]
    async fn session_without_a_token_is_unauthorized() {
        // Act
        let result = refresh(
            StubOutcome::Verified(app_token(Some(""), None)),
            None,
            &jwt(),
        )
        .await;

        // Assert
        assert_eq!(code(result), RequestErrorCode::Unauthorized);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn track_status_carrying_a_token_is_answered_with_ok() {
        // Arrange
        let (_relay, client) = spawn_relay_and_connect_client(app_token(Some(""), None)).await;

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
}
