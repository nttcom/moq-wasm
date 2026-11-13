use auth_token::Access;
use hyper::{HeaderMap, StatusCode, header::AUTHORIZATION};

use crate::{
    ANONYMOUS_APP_ID,
    app_verifier::{AppVerifier, VerifyError},
};

/// Why a request may not read stats.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The `Authorization` header is not `Bearer <token>`, or the VTS or the
    /// claim check rejected the token.
    Unauthorized,
    /// The token's `subscribe` claim does not cover its whole app.
    Forbidden,
    /// The token could not be verified: no VTS is configured or it failed.
    Unavailable,
}

impl Refusal {
    /// The status and error code it answers with.
    pub(crate) fn response(&self) -> (StatusCode, &'static str) {
        match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "token_verification_unavailable",
            ),
        }
    }
}

/// The app whose stats a request may read: `anon` without a token, else the
/// app of a verified token whose `subscribe` claim covers the whole app
/// (`""`), since a session belongs to an app, not to a namespace below it.
pub(crate) async fn viewer_app(
    headers: &HeaderMap,
    verifier: Option<&AppVerifier>,
) -> Result<String, Refusal> {
    let Some(authorization) = headers.get(AUTHORIZATION) else {
        return Ok(ANONYMOUS_APP_ID.to_string());
    };
    let token = authorization
        .to_str()
        .ok()
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty())
        .ok_or(Refusal::Unauthorized)?;
    let verifier = verifier.ok_or(Refusal::Unavailable)?;
    let signed = verifier.verify(token).await.map_err(|error| match error {
        VerifyError::Unauthorized(_) => {
            tracing::debug!(%error, "stats request rejected");
            Refusal::Unauthorized
        }
        VerifyError::Unavailable(_) => {
            tracing::warn!(%error, "stats request was not verified");
            Refusal::Unavailable
        }
    })?;
    if !signed.covers(Access::Subscribe, &signed.app_id) {
        return Err(Refusal::Forbidden);
    }
    Ok(signed.app_id)
}

#[cfg(test)]
mod tests {
    use hyper::{HeaderMap, StatusCode, header::AUTHORIZATION};
    use tokio::sync::mpsc;

    use super::{Refusal, viewer_app};
    use crate::{
        app_verifier::AppVerifier,
        test_support::{fake_http_server_answering, vts_answer},
    };

    fn bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        headers
    }

    /// The VTS's request receiver must outlive the verifier's requests.
    async fn verifier_answering(
        status: StatusCode,
        body: &str,
    ) -> (AppVerifier, mpsc::UnboundedReceiver<(String, String)>) {
        let (vts_url, vts_requests) = fake_http_server_answering(status, body).await;
        (
            AppVerifier::new(format!("{vts_url}/verify")).unwrap(),
            vts_requests,
        )
    }

    #[tokio::test]
    async fn a_request_without_a_token_reads_the_anonymous_app() {
        // Act / Assert
        assert_eq!(
            viewer_app(&HeaderMap::new(), None).await,
            Ok("anon".to_string())
        );
    }

    #[tokio::test]
    async fn a_token_covering_its_whole_app_reads_that_app() {
        // Arrange
        let (verifier, _verifier_requests) =
            verifier_answering(StatusCode::OK, &vts_answer("app-1", Some(""))).await;

        // Act / Assert
        assert_eq!(
            viewer_app(&bearer("jwt"), Some(&verifier)).await,
            Ok("app-1".to_string())
        );
    }

    #[tokio::test]
    async fn a_token_for_part_of_its_app_or_for_publishing_only_is_forbidden() {
        // Arrange
        let (room, _room_requests) =
            verifier_answering(StatusCode::OK, &vts_answer("app-1", Some("room"))).await;
        let (publisher, _publisher_requests) =
            verifier_answering(StatusCode::OK, &vts_answer("app-1", None)).await;

        // Act / Assert
        assert_eq!(
            viewer_app(&bearer("jwt"), Some(&room)).await,
            Err(Refusal::Forbidden)
        );
        assert_eq!(
            viewer_app(&bearer("jwt"), Some(&publisher)).await,
            Err(Refusal::Forbidden)
        );
    }

    #[tokio::test]
    async fn a_rejected_or_malformed_token_is_unauthorized() {
        // Arrange
        let (verifier, _verifier_requests) =
            verifier_answering(StatusCode::UNAUTHORIZED, r#"{"error":"bad"}"#).await;
        let mut basic = HeaderMap::new();
        basic.insert(AUTHORIZATION, "Basic abc".parse().unwrap());

        // Act / Assert
        assert_eq!(
            viewer_app(&bearer("jwt"), Some(&verifier)).await,
            Err(Refusal::Unauthorized)
        );
        assert_eq!(
            viewer_app(&basic, Some(&verifier)).await,
            Err(Refusal::Unauthorized)
        );
    }

    #[tokio::test]
    async fn a_token_without_a_vts_cannot_be_verified() {
        // Act / Assert
        assert_eq!(
            viewer_app(&bearer("jwt"), None).await,
            Err(Refusal::Unavailable)
        );
    }
}
