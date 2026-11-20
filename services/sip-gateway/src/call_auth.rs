use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use app_registry::{Apps, RegistryStore, is_e164};
use auth_token::{Access, ClaimPolicy, SignedToken};
use http_server::bearer_token;
use hyper::{HeaderMap, StatusCode};

/// A call token lives at most this long: an app server signs one per call.
const CALL_TOKEN_POLICY: ClaimPolicy = ClaimPolicy {
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

/// The app a call request was authorized for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AuthorizedCall {
    pub(crate) app_id: String,
    /// The app's secret, which signs the gateway's relay token; `None`
    /// without authorization.
    pub(crate) secret: Option<String>,
    /// The caller the INVITE shows: the app's first number, if it has one.
    pub(crate) from: Option<String>,
}

/// Verifies the call token an app server sends as `Authorization: Bearer`
/// through the VTS, which checks only its signature, and the destination
/// against the app's phone settings in the registry.
pub(crate) enum CallAuth {
    Vts {
        client: reqwest::Client,
        vts_url: String,
        registry: Arc<RegistryStore>,
    },
    /// Accepts every request for the app the prefix starts with and joins
    /// the relay with no token (`SIP_GATEWAY_INSECURE_SKIP_AUTH`, a local
    /// relay without authorization only).
    Insecure,
}

impl CallAuth {
    pub(crate) fn new(vts_url: String, registry: Arc<RegistryStore>) -> Self {
        Self::Vts {
            client: reqwest::Client::new(),
            vts_url,
            registry,
        }
    }

    pub(crate) async fn authorize(
        &self,
        headers: &HeaderMap,
        namespace_prefix: &str,
        to: &str,
    ) -> Result<AuthorizedCall, Rejection> {
        if !is_e164(to) {
            return Err(rejected(StatusCode::BAD_REQUEST, "invalid_destination"));
        }
        let Self::Vts {
            client,
            vts_url,
            registry,
        } = self
        else {
            return Ok(AuthorizedCall {
                app_id: namespace_prefix
                    .split('/')
                    .next()
                    .unwrap_or_default()
                    .to_string(),
                secret: None,
                from: None,
            });
        };
        let token = bearer_token(headers)
            .ok_or_else(|| rejected(StatusCode::UNAUTHORIZED, "a bearer token is required"))?;
        let response = client
            .post(vts_url)
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
        let apps = registry
            .load()
            .await
            .map_err(|_| rejected(StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable"))?;
        app_call(&apps, &verified.app_id, to)
    }
}

/// The registered app's call to `to`: refused unless its phone settings
/// allow the destination.
fn app_call(apps: &Apps, app_id: &str, to: &str) -> Result<AuthorizedCall, Rejection> {
    let app = apps
        .get(app_id)
        .ok_or_else(|| rejected(StatusCode::FORBIDDEN, "forbidden"))?;
    let phone = app
        .phone
        .as_ref()
        .filter(|phone| phone.allows_destination(to))
        .ok_or_else(|| rejected(StatusCode::FORBIDDEN, "destination_not_allowed"))?;
    Ok(AuthorizedCall {
        app_id: app_id.to_string(),
        secret: Some(app.secret.clone()),
        from: phone.numbers.first().cloned(),
    })
}

fn check(verified: &SignedToken, namespace_prefix: &str, now: SystemTime) -> Result<(), Rejection> {
    verified
        .check(CALL_TOKEN_POLICY, now)
        .map_err(|_| rejected(StatusCode::UNAUTHORIZED, "invalid_token_lifetime"))?;
    if verified.is_relay || !verified.covers(Access::Calls, namespace_prefix) {
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

    fn verified(is_relay: bool, calls: Option<&str>, exp: u64) -> SignedToken {
        serde_json::from_value(json!({
            "appId": "APP",
            "isRelay": is_relay,
            "claims": { "appId": "APP", "calls": calls, "bots": "", "iat": 100, "exp": exp },
        }))
        .unwrap()
    }

    #[test]
    fn only_a_short_lived_calls_claim_covering_the_room_passes() {
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
    async fn a_destination_that_is_not_e164_is_refused_before_the_token_is_read() {
        // Act
        let rejection = CallAuth::Insecure
            .authorize(&HeaderMap::new(), "anon/room", "0312345678")
            .await;

        // Assert
        assert_eq!(rejection.unwrap_err().error, "invalid_destination");
    }

    #[tokio::test]
    async fn without_authorization_the_app_is_the_rooms_first_element() {
        // Act
        let call = CallAuth::Insecure
            .authorize(&HeaderMap::new(), "anon/room", "+81312345678")
            .await;

        // Assert
        assert_eq!(
            call,
            Ok(AuthorizedCall {
                app_id: "anon".to_string(),
                secret: None,
                from: None,
            })
        );
    }

    #[test]
    fn the_app_s_phone_settings_decide_the_destination_and_the_caller() {
        // Arrange
        let apps = Apps::new(
            serde_json::from_value(json!([
                {
                    "appId": "app",
                    "secret": "app-secret",
                    "isRelay": false,
                    "phone": {
                        "numbers": ["+81300000000"],
                        "callWebhook": "https://app.example/calls",
                        "allowedDestinations": ["+8190"],
                    },
                },
                { "appId": "silent", "secret": "silent-secret", "isRelay": false },
            ]))
            .unwrap(),
        )
        .unwrap();

        // Act / Assert
        assert_eq!(
            app_call(&apps, "app", "+819012345678"),
            Ok(AuthorizedCall {
                app_id: "app".to_string(),
                secret: Some("app-secret".to_string()),
                from: Some("+81300000000".to_string()),
            })
        );
        assert_eq!(
            app_call(&apps, "app", "+12025550100").unwrap_err().error,
            "destination_not_allowed"
        );
        assert_eq!(
            app_call(&apps, "silent", "+819012345678")
                .unwrap_err()
                .error,
            "destination_not_allowed"
        );
        assert_eq!(
            app_call(&apps, "gone", "+819012345678").unwrap_err().error,
            "forbidden"
        );
    }
}
