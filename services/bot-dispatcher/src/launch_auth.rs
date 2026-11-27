use std::time::Duration;

use auth_token::{Access, AuthorizeError, ClaimPolicy, VtsClient};
use http_server::bearer_token;
use hyper::{HeaderMap, StatusCode};

/// A launch token lives at most this long: an app server signs one per call.
const LAUNCH_TOKEN_POLICY: ClaimPolicy = ClaimPolicy {
    max_token_ttl: Duration::from_secs(5 * 60),
    clock_leeway: Duration::from_secs(60),
};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Rejection {
    pub(crate) status: StatusCode,
    pub(crate) error: &'static str,
}

pub(crate) fn rejected(status: StatusCode, error: &'static str) -> Rejection {
    Rejection { status, error }
}

/// Verifies the launch token an app server sends as `Authorization: Bearer`
/// through the VTS, which checks only its signature.
pub(crate) enum LaunchAuth {
    Vts(VtsClient),
    /// Accepts every request for the app the prefix starts with
    /// (`BOT_DISPATCHER_INSECURE_SKIP_AUTH`, a relay without authorization only).
    Insecure,
}

impl LaunchAuth {
    pub(crate) fn new(vts_url: String) -> Self {
        Self::Vts(VtsClient::new(vts_url))
    }

    /// The app of the verified token, whose `bots` claim covers
    /// `namespace_prefix`.
    pub(crate) async fn authorize(
        &self,
        headers: &HeaderMap,
        namespace_prefix: &str,
    ) -> Result<String, Rejection> {
        let Self::Vts(vts) = self else {
            return Ok(namespace_prefix
                .split('/')
                .next()
                .unwrap_or_default()
                .to_string());
        };
        let verified = vts
            .authorize(
                bearer_token(headers),
                Access::Bots,
                namespace_prefix,
                LAUNCH_TOKEN_POLICY,
            )
            .await
            .map_err(rejection)?;
        Ok(verified.app_id)
    }
}

fn rejection(error: AuthorizeError) -> Rejection {
    match error {
        AuthorizeError::MissingToken => {
            rejected(StatusCode::UNAUTHORIZED, "a bearer token is required")
        }
        AuthorizeError::Unauthorized(_) => rejected(StatusCode::UNAUTHORIZED, "invalid_token"),
        AuthorizeError::InvalidClaims(_) => {
            rejected(StatusCode::UNAUTHORIZED, "invalid_token_lifetime")
        }
        AuthorizeError::Forbidden => rejected(StatusCode::FORBIDDEN, "forbidden"),
        _ => rejected(StatusCode::SERVICE_UNAVAILABLE, "vts_unavailable"),
    }
}

#[cfg(test)]
mod tests {
    use auth_token::ClaimError;

    use super::*;

    #[test]
    fn each_authorization_failure_keeps_its_status_and_error() {
        // Act / Assert
        assert_eq!(
            [
                AuthorizeError::MissingToken,
                AuthorizeError::Unauthorized("bad".to_string()),
                AuthorizeError::Unavailable(anyhow::anyhow!("down")),
                AuthorizeError::InvalidClaims(ClaimError::Expired),
                AuthorizeError::Forbidden,
            ]
            .map(rejection),
            [
                rejected(StatusCode::UNAUTHORIZED, "a bearer token is required"),
                rejected(StatusCode::UNAUTHORIZED, "invalid_token"),
                rejected(StatusCode::SERVICE_UNAVAILABLE, "vts_unavailable"),
                rejected(StatusCode::UNAUTHORIZED, "invalid_token_lifetime"),
                rejected(StatusCode::FORBIDDEN, "forbidden"),
            ]
        );
    }

    #[tokio::test]
    async fn without_authorization_the_app_is_the_rooms_first_element() {
        // Act
        let app_id = LaunchAuth::Insecure
            .authorize(&HeaderMap::new(), "anon/room")
            .await;

        // Assert
        assert_eq!(app_id, Ok("anon".to_string()));
    }
}
