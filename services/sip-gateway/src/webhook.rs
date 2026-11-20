use std::time::Duration;

use anyhow::{Context, Result, bail};
use auth_token::sign_token;
use serde::{Deserialize, Serialize};

/// The webhook token outlives the longest the gateway waits for an answer.
const TOKEN_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// An INVITE to one of an app's numbers, as its call webhook receives it.
#[derive(Serialize)]
pub(crate) struct IncomingCall {
    pub(crate) call_id: String,
    pub(crate) from: String,
    pub(crate) to: String,
}

/// The app server's answer to an incoming call.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub(crate) enum Decision {
    /// Bridge the call into the room `namespace_prefix` with `peer`, as an
    /// outbound call's `POST /calls`.
    Answer {
        namespace_prefix: String,
        peer: String,
    },
    Reject {
        reject: Refusal,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Refusal {
    /// 486 Busy Here.
    Busy,
    /// 603 Decline.
    Decline,
}

impl Refusal {
    pub(crate) fn sip_status(self) -> u16 {
        match self {
            Self::Busy => 486,
            Self::Decline => 603,
        }
    }
}

/// The claims of the token a webhook request carries as `Authorization:
/// Bearer`, signed with the app's secret (HS256), so the app server knows
/// the gateway sent it and which call it is about.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WebhookClaims<'a> {
    app_id: &'a str,
    call_id: &'a str,
    from: &'a str,
    to: &'a str,
    iat: u64,
    exp: u64,
}

#[derive(Default)]
pub(crate) struct CallWebhook {
    client: reqwest::Client,
}

impl CallWebhook {
    /// POSTs `call` to `url` and reads the decision. Fails when the server
    /// cannot be reached, does not answer 2xx, or answers anything but a
    /// decision.
    pub(crate) async fn ask(
        &self,
        url: &str,
        app_id: &str,
        secret: &str,
        call: &IncomingCall,
        now: u64,
    ) -> Result<Decision> {
        let token = webhook_token(app_id, secret, call, now)?;
        let response = self
            .client
            .post(url)
            .bearer_auth(token)
            .json(call)
            .send()
            .await
            .context("call the webhook")?;
        let status = response.status();
        if !status.is_success() {
            bail!("the webhook answered {status}");
        }
        response.json().await.context("read the webhook's decision")
    }
}

fn webhook_token(app_id: &str, secret: &str, call: &IncomingCall, now: u64) -> Result<String> {
    sign_token(
        &WebhookClaims {
            app_id,
            call_id: &call.call_id,
            from: &call.from,
            to: &call.to,
            iat: now,
            exp: now + TOKEN_LIFETIME.as_secs(),
        },
        secret,
    )
}

#[cfg(test)]
mod tests {
    use auth_token::{decode_unverified_payload, verify_signature};
    use serde_json::json;

    use super::*;

    #[test]
    fn the_token_names_the_app_and_the_call_signed_with_the_app_s_secret() {
        // Arrange
        let call = IncomingCall {
            call_id: "abc".to_string(),
            from: "+819012345678".to_string(),
            to: "+81300000000".to_string(),
        };

        // Act
        let token = webhook_token("app", "app-secret", &call, 100).unwrap();

        // Assert
        verify_signature(&token, "app-secret").unwrap();
        let payload = decode_unverified_payload(&token).unwrap();
        assert_eq!(
            serde_json::Value::Object(payload),
            json!({
                "appId": "app",
                "callId": "abc",
                "from": "+819012345678",
                "to": "+81300000000",
                "iat": 100,
                "exp": 400,
            })
        );
    }

    #[test]
    fn a_decision_answers_with_the_room_or_refuses() {
        // Act / Assert
        assert_eq!(
            serde_json::from_value::<Decision>(
                json!({ "namespace_prefix": "app/room", "peer": "app/room/alice/call" })
            )
            .unwrap(),
            Decision::Answer {
                namespace_prefix: "app/room".to_string(),
                peer: "app/room/alice/call".to_string(),
            }
        );
        assert_eq!(
            serde_json::from_value::<Decision>(json!({ "reject": "busy" })).unwrap(),
            Decision::Reject {
                reject: Refusal::Busy
            }
        );
        assert!(serde_json::from_value::<Decision>(json!({ "reject": "maybe" })).is_err());
        assert!(serde_json::from_value::<Decision>(json!({})).is_err());
    }
}
