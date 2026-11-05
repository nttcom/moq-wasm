use std::{sync::LazyLock, time::Duration};

use auth_token::Claims;
use hyper::{HeaderMap, StatusCode, header::AUTHORIZATION};
use serde_json::Value;

use crate::state_sync_task::now_ms;

/// The clock difference the relay tolerates on `iat` and `exp`, so a token
/// the relay accepts is accepted here too.
const CLOCK_LEEWAY_SECONDS: u64 = 60;
const MAX_CLIENT_TOKEN_SECONDS: u64 = 86_400;

static VTS_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

#[derive(Debug)]
pub(crate) struct AuthError {
    pub(crate) status: StatusCode,
    pub(crate) message: &'static str,
}

/// The anonymous app is public, matching the relay. Private apps require VTS
/// verification and a namespace claim covering the requested operation.
pub(crate) async fn authorize(
    headers: &HeaderMap,
    vts: Option<&str>,
    namespace: &str,
    write: bool,
) -> Result<(), AuthError> {
    if headers.get(AUTHORIZATION).is_none() {
        return if namespace == "anon" || namespace.starts_with("anon/") {
            Ok(())
        } else {
            Err(rejected(
                StatusCode::UNAUTHORIZED,
                "a bearer token is required",
            ))
        };
    }
    let token = bearer_token(headers)
        .ok_or_else(|| rejected(StatusCode::UNAUTHORIZED, "invalid bearer token"))?;
    let vts = vts.ok_or_else(|| {
        rejected(
            StatusCode::SERVICE_UNAVAILABLE,
            "HTTP token verification is not configured",
        )
    })?;
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
    let verified: Value = response.json().await.map_err(|_| {
        rejected(
            StatusCode::SERVICE_UNAVAILABLE,
            "invalid token verification response",
        )
    })?;
    check_claims(&verified, namespace, write, now_ms() / 1000)
}

pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|value| !value.is_empty())
}

fn check_claims(verified: &Value, namespace: &str, write: bool, now: u64) -> Result<(), AuthError> {
    let denied = || rejected(StatusCode::FORBIDDEN, "token does not authorize this room");
    let app_id = verified["appId"].as_str().ok_or_else(denied)?;
    let claims: Claims =
        serde_json::from_value(verified["claims"].clone()).map_err(|_| denied())?;
    if claims.app_id != app_id {
        return Err(denied());
    }
    let iat = claims.iat.ok_or_else(denied)?;
    let exp = claims.exp.ok_or_else(denied)?;
    if exp.saturating_add(CLOCK_LEEWAY_SECONDS) <= now
        || iat > now.saturating_add(CLOCK_LEEWAY_SECONDS)
        || exp <= iat
        || (verified["isRelay"] != true && exp.saturating_sub(iat) > MAX_CLIENT_TOKEN_SECONDS)
    {
        return Err(rejected(
            StatusCode::UNAUTHORIZED,
            "token lifetime is invalid",
        ));
    }
    let claim = if write {
        claims.publish
    } else {
        claims.subscribe
    }
    .ok_or_else(denied)?;
    if !claim.is_empty() && claim.split('/').any(str::is_empty) {
        return Err(denied());
    }
    let allowed = if claim.is_empty() {
        app_id.to_string()
    } else {
        format!("{app_id}/{claim}")
    };
    if namespace == allowed
        || namespace
            .strip_prefix(&allowed)
            .is_some_and(|suffix| suffix.starts_with('/'))
    {
        Ok(())
    } else {
        Err(denied())
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
    async fn private_rooms_require_a_successful_vts_response() {
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
            authorize(&HeaderMap::new(), Some(&url), "APP/room", false)
                .await
                .unwrap_err()
                .status,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            authorize(&headers, Some(&url), "APP/room", false)
                .await
                .unwrap_err()
                .status,
            StatusCode::UNAUTHORIZED
        );
        server.await.unwrap();
        assert_eq!(
            authorize(&headers, Some(&url), "APP/room", false)
                .await
                .unwrap_err()
                .status,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn scopes_are_checked_on_component_boundaries_and_expired_tokens_fail() {
        // Arrange
        let mut verified = json!({ "appId": "APP", "isRelay": false, "claims": { "appId": "APP", "subscribe": "room", "iat": 90, "exp": 150 } });
        // Act / Assert
        assert!(check_claims(&verified, "APP/room", false, 100).is_ok());
        assert!(check_claims(&verified, "APP/room/recordings/id", false, 100).is_ok());
        assert!(check_claims(&verified, "APP/room2", false, 100).is_err());
        assert!(check_claims(&verified, "OTHER/room", false, 100).is_err());
        assert!(check_claims(&verified, "APP/room", true, 100).is_err());
        verified["claims"]["exp"] = json!(99);
        assert!(check_claims(&verified, "APP/room", false, 100).is_ok());
        verified["claims"]["exp"] = json!(40);
        assert!(check_claims(&verified, "APP/room", false, 100).is_err());
    }
}
