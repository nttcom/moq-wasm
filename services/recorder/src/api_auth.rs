use std::{
    sync::LazyLock,
    time::{Duration, SystemTime},
};

use auth_token::{Access, ClaimError, ClaimPolicy, SignedToken};
use http_server::bearer_token;
use hyper::{HeaderMap, StatusCode, header::AUTHORIZATION};

const DENIED: &str = "token does not authorize this room";

static VTS_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

#[derive(Debug)]
pub(crate) struct AuthError {
    pub(crate) status: StatusCode,
    pub(crate) message: &'static str,
}

/// Requires a bearer token the VTS verifies whose `subscribe` claim covers
/// `namespace`.
pub(crate) async fn authorize(
    headers: &HeaderMap,
    vts: &str,
    namespace: &str,
) -> Result<(), AuthError> {
    if headers.get(AUTHORIZATION).is_none() {
        return Err(rejected(
            StatusCode::UNAUTHORIZED,
            "a bearer token is required",
        ));
    }
    let token = bearer_token(headers)
        .ok_or_else(|| rejected(StatusCode::UNAUTHORIZED, "invalid bearer token"))?;
    let response = VTS_CLIENT
        .post(vts)
        .timeout(Duration::from_secs(3))
        .json(&serde_json::json!({ "token": token }))
        .send()
        .await
        .map_err(|_| {
            rejected(
                StatusCode::SERVICE_UNAVAILABLE,
                "token verification unavailable",
            )
        })?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(rejected(
            StatusCode::UNAUTHORIZED,
            "token verification rejected",
        ));
    }
    if !response.status().is_success() {
        return Err(rejected(
            StatusCode::SERVICE_UNAVAILABLE,
            "token verification unavailable",
        ));
    }
    let verified: SignedToken = response.json().await.map_err(|_| {
        rejected(
            StatusCode::SERVICE_UNAVAILABLE,
            "invalid token verification response",
        )
    })?;
    check_claims(&verified, namespace, SystemTime::now())
}

fn check_claims(verified: &SignedToken, namespace: &str, now: SystemTime) -> Result<(), AuthError> {
    verified
        .check(ClaimPolicy::default(), now)
        .map_err(|error| match error {
            ClaimError::EmptyElement => rejected(StatusCode::FORBIDDEN, DENIED),
            _ => rejected(StatusCode::UNAUTHORIZED, "token lifetime is invalid"),
        })?;
    if verified.covers(Access::Subscribe, namespace) {
        Ok(())
    } else {
        Err(rejected(StatusCode::FORBIDDEN, DENIED))
    }
}

fn rejected(status: StatusCode, message: &'static str) -> AuthError {
    AuthError { status, message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn every_room_requires_a_successful_vts_response() {
        // Arrange
        let server = http_server::testing::StubServer::run(StatusCode::UNAUTHORIZED, "{}").await;
        let url = format!("{}/verify", server.url());
        let closed_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unreachable_url = format!("http://{}/verify", closed_listener.local_addr().unwrap());
        drop(closed_listener);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "Bearer invalid".parse().unwrap());

        // Act / Assert
        assert_eq!(
            authorize(&HeaderMap::new(), &url, "anon/room")
                .await
                .unwrap_err()
                .status,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            authorize(&headers, &url, "APP/room")
                .await
                .unwrap_err()
                .status,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            authorize(&headers, &unreachable_url, "APP/room")
                .await
                .unwrap_err()
                .status,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn scopes_are_checked_on_component_boundaries_and_expired_tokens_fail() {
        // Arrange
        let at = |secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
        let mut verified: SignedToken = serde_json::from_value(json!({ "appId": "APP", "isRelay": false, "claims": { "appId": "APP", "subscribe": "room", "iat": 90, "exp": 150 } })).unwrap();

        // Act / Assert
        assert!(check_claims(&verified, "APP/room", at(100)).is_ok());
        assert!(check_claims(&verified, "APP/room/recordings/id", at(100)).is_ok());
        assert!(check_claims(&verified, "APP/room2", at(100)).is_err());
        assert!(check_claims(&verified, "OTHER/room", at(100)).is_err());
        verified.claims.exp = Some(99);
        assert!(check_claims(&verified, "APP/room", at(100)).is_ok());
        verified.claims.exp = Some(40);
        assert_eq!(
            check_claims(&verified, "APP/room", at(100))
                .unwrap_err()
                .status,
            StatusCode::UNAUTHORIZED
        );
    }
}
