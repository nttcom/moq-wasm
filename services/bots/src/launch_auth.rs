use std::time::{Duration, SystemTime};

use auth_token::{Access, ClaimPolicy, SignedToken};
use hyper::{HeaderMap, StatusCode, header::AUTHORIZATION};

/// A launch token lives at most this long: an app server signs one per call.
const LAUNCH_TOKEN_POLICY: ClaimPolicy = ClaimPolicy {
    max_token_ttl: Duration::from_secs(5 * 60),
    clock_leeway: Duration::from_secs(60),
};
const VTS_TIMEOUT: Duration = Duration::from_secs(3);

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
pub(crate) struct LaunchAuth {
    client: reqwest::Client,
    vts_url: String,
}

impl LaunchAuth {
    pub(crate) fn new(vts_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            vts_url,
        }
    }

    /// The verified token, whose `bots` claim covers `namespace_prefix`.
    pub(crate) async fn authorize(
        &self,
        headers: &HeaderMap,
        namespace_prefix: &str,
    ) -> Result<SignedToken, Rejection> {
        let token = bearer_token(headers)
            .ok_or_else(|| rejected(StatusCode::UNAUTHORIZED, "a bearer token is required"))?;
        let response = self
            .client
            .post(&self.vts_url)
            .timeout(VTS_TIMEOUT)
            .json(&serde_json::json!({ "token": token }))
            .send()
            .await
            .map_err(|_| rejected(StatusCode::SERVICE_UNAVAILABLE, "vts_unavailable"))?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(rejected(StatusCode::UNAUTHORIZED, "invalid_token"));
        }
        if !response.status().is_success() {
            return Err(rejected(StatusCode::SERVICE_UNAVAILABLE, "vts_unavailable"));
        }
        let verified: SignedToken = response
            .json()
            .await
            .map_err(|_| rejected(StatusCode::SERVICE_UNAVAILABLE, "vts_unavailable"))?;
        check(&verified, namespace_prefix, SystemTime::now())?;
        Ok(verified)
    }
}

pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|value| !value.is_empty())
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
}
