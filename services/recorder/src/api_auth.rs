use std::{
    sync::LazyLock,
    time::{Duration, SystemTime},
};

use auth_token::{Access, ClaimError, ClaimPolicy, SignedToken};
use hyper::{HeaderMap, StatusCode, header::AUTHORIZATION};

const DENIED: &str = "token does not authorize this room";

static VTS_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

#[derive(Debug)]
pub(crate) struct AuthError {
    pub(crate) status: StatusCode,
    pub(crate) message: &'static str,
}

/// Requires a bearer token the VTS verifies whose `publish` claim (`write`)
/// or `subscribe` claim covers `namespace`.
pub(crate) async fn authorize(
    headers: &HeaderMap,
    vts: &str,
    namespace: &str,
    write: bool,
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
    check_claims(&verified, namespace, write, SystemTime::now())
}

pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|value| !value.is_empty())
}

fn check_claims(
    verified: &SignedToken,
    namespace: &str,
    write: bool,
    now: SystemTime,
) -> Result<(), AuthError> {
    verified
        .check(ClaimPolicy::default(), now)
        .map_err(|error| match error {
            ClaimError::EmptyElement => rejected(StatusCode::FORBIDDEN, DENIED),
            _ => rejected(StatusCode::UNAUTHORIZED, "token lifetime is invalid"),
        })?;
    let access = if write {
        Access::Publish
    } else {
        Access::Subscribe
    };
    if verified.covers(access, namespace) {
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/verify", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let service =
                hyper::service::service_fn(|_: hyper::Request<hyper::body::Incoming>| async {
                    let mut response = hyper::Response::new(http_body_util::Full::new(
                        bytes::Bytes::from_static(b"{}"),
                    ));
                    *response.status_mut() = StatusCode::UNAUTHORIZED;
                    Ok::<_, std::convert::Infallible>(response)
                });
            hyper::server::conn::http1::Builder::new()
                .keep_alive(false)
                .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                .await
                .unwrap();
        });
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "Bearer invalid".parse().unwrap());

        // Act / Assert
        assert_eq!(
            authorize(&HeaderMap::new(), &url, "anon/room", false)
                .await
                .unwrap_err()
                .status,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            authorize(&headers, &url, "APP/room", false)
                .await
                .unwrap_err()
                .status,
            StatusCode::UNAUTHORIZED
        );
        server.await.unwrap();
        assert_eq!(
            authorize(&headers, &url, "APP/room", false)
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
        assert!(check_claims(&verified, "APP/room", false, at(100)).is_ok());
        assert!(check_claims(&verified, "APP/room/recordings/id", false, at(100)).is_ok());
        assert!(check_claims(&verified, "APP/room2", false, at(100)).is_err());
        assert!(check_claims(&verified, "OTHER/room", false, at(100)).is_err());
        assert!(check_claims(&verified, "APP/room", true, at(100)).is_err());
        verified.claims.exp = Some(99);
        assert!(check_claims(&verified, "APP/room", false, at(100)).is_ok());
        verified.claims.exp = Some(40);
        assert_eq!(
            check_claims(&verified, "APP/room", false, at(100))
                .unwrap_err()
                .status,
            StatusCode::UNAUTHORIZED
        );
    }
}
