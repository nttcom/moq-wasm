use std::{sync::Arc, time::Duration};

use app_registry::{Apps, RegistryStore, is_e164};
use auth_token::{Access, AuthorizeError, ClaimPolicy, VtsClient};
use http_server::bearer_token;
use hyper::{HeaderMap, StatusCode};

/// A call token lives at most this long: an app server signs one per call.
const CALL_TOKEN_POLICY: ClaimPolicy = ClaimPolicy {
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
        vts: VtsClient,
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
            vts: VtsClient::new(vts_url),
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
        let Self::Vts { vts, registry } = self else {
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
        let verified = vts
            .authorize(
                bearer_token(headers),
                Access::Calls,
                namespace_prefix,
                CALL_TOKEN_POLICY,
            )
            .await
            .map_err(rejection)?;
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
    use serde_json::json;

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
