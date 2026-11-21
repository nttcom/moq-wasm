use std::time::{Duration, SystemTime};

use auth_token::{Access, ClaimPolicy, SignedToken, VtsClient, VtsVerifyError};
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

fn rejected(status: StatusCode, error: &'static str) -> Rejection {
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
        let token = bearer_token(headers)
            .ok_or_else(|| rejected(StatusCode::UNAUTHORIZED, "a bearer token is required"))?;
        let verified = vts.verify(token).await.map_err(|error| match error {
            VtsVerifyError::Unauthorized(_) => rejected(StatusCode::UNAUTHORIZED, "invalid_token"),
            _ => rejected(StatusCode::SERVICE_UNAVAILABLE, "vts_unavailable"),
        })?;
        check(&verified, namespace_prefix, SystemTime::now())?;
        Ok(verified.app_id)
    }
}

fn check(verified: &SignedToken, namespace_prefix: &str, now: SystemTime) -> Result<(), Rejection> {
    verified
        .check(LAUNCH_TOKEN_POLICY, now)
        .map_err(|_| rejected(StatusCode::UNAUTHORIZED, "invalid_token_lifetime"))?;
    if verified.is_relay || !verified.covers(Access::Bots, namespace_prefix) {
        return Err(rejected(StatusCode::FORBIDDEN, "forbidden"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn verified(is_relay: bool, bots: Option<&str>, exp: u64) -> SignedToken {
        serde_json::from_value(json!({
            "appId": "APP",
            "isRelay": is_relay,
            "claims": { "appId": "APP", "bots": bots, "subscribe": "", "iat": 100, "exp": exp },
        }))
        .unwrap()
    }

    #[test]
    fn only_a_short_lived_bots_claim_covering_the_room_passes() {
        // Act / Assert
        assert_eq!(
            check(&verified(false, Some("room"), 400), "APP/room", at(110)),
            Ok(())
        );
        assert_eq!(
            check(&verified(false, Some("room"), 400), "APP/room2", at(110))
                .unwrap_err()
                .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            check(&verified(false, None, 400), "APP/room", at(110))
                .unwrap_err()
                .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            check(&verified(true, Some(""), 400), "APP/room", at(110))
                .unwrap_err()
                .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            check(
                &verified(false, Some("room"), 100 + 3600),
                "APP/room",
                at(110)
            )
            .unwrap_err()
            .status,
            StatusCode::UNAUTHORIZED
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
