use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use serde_json::{Value, json};

/// The environment variable `bots` passes a locally launched bot its key in.
const BOT_KEY_ENV: &str = "BOTS_BOT_KEY";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// What a bot reports to `bots` at every heartbeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BotState {
    Running,
    Stopping,
    Finished,
    Failed,
}

impl BotState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Finished => "finished",
            Self::Failed => "failed",
        }
    }
}

/// The calls a recording started by `bots` makes back to it
/// (docs/architecture/bots/architecture.md): its token, and its heartbeat.
/// It authenticates with the key `bots` gave it in `BOTS_BOT_KEY` when it was
/// launched locally, and otherwise with the Google ID token of the job's
/// service account for the bots URL.
pub(crate) struct BotsClient {
    http: reqwest::Client,
    url: String,
    bot_type: String,
    id: String,
    key: Option<String>,
}

impl BotsClient {
    pub(crate) fn from_env(url: String, bot_type: String, id: String) -> Self {
        let key = std::env::var(BOT_KEY_ENV)
            .ok()
            .filter(|key| !key.is_empty());
        Self::new(url, bot_type, id, key)
    }

    fn new(url: String, bot_type: String, id: String, key: Option<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            url: url.trim_end_matches('/').to_string(),
            bot_type,
            id,
            key,
        }
    }

    /// The type `bots` started the bot as, the last element of the namespace
    /// its token may publish: `<room>/bots/<type>`.
    pub(crate) fn bot_type(&self) -> &str {
        &self.bot_type
    }

    /// A new token for the relay; `None` from a `bots` without authorization,
    /// whose bots join with no token. Fails when `bots` refuses, as it does
    /// once the bot has ended.
    pub(crate) async fn token(&self) -> Result<Option<String>> {
        let (status, body) = self.post("token", None).await?;
        if !status.is_success() {
            bail!("bots answered {status}: {}", body["error"]);
        }
        match &body["token"] {
            Value::String(token) => Ok(Some(token.clone())),
            Value::Null => Ok(None),
            _ => bail!("bots answered without a token"),
        }
    }

    /// `true` when `bots` asks the bot to stop, or takes it as ended.
    pub(crate) async fn heartbeat(&self, state: BotState, error: Option<&str>) -> Result<bool> {
        let body = json!({ "state": state.as_str(), "error": error });
        let (status, body) = self.post("heartbeat", Some(body)).await?;
        if status == StatusCode::GONE {
            return Ok(true);
        }
        if !status.is_success() {
            bail!("bots answered {status}: {}", body["error"]);
        }
        Ok(body["stop"].as_bool().unwrap_or(false))
    }

    async fn post(&self, call: &str, body: Option<Value>) -> Result<(StatusCode, Value)> {
        let bearer = match &self.key {
            Some(key) => key.clone(),
            None => google_cloud::identity_token(&self.http, &self.url)
                .await
                .context("cannot get an ID token from the metadata server")?,
        };
        let url = format!("{}/bots/{}/{}/{call}", self.url, self.bot_type, self.id);
        let mut request = self
            .http
            .post(&url)
            .timeout(REQUEST_TIMEOUT)
            .bearer_auth(bearer);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("cannot reach {url}"))?;
        let status = response.status();
        Ok((status, response.json().await.unwrap_or_default()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use http_server::{HttpServer, error_response, json_response};
    use hyper::{StatusCode as HttpStatus, header::AUTHORIZATION};
    use tokio::net::TcpListener;

    use super::*;

    type Calls = Arc<Mutex<Vec<(String, String)>>>;

    /// A `bots` that answers every heartbeat `410 Gone` and every token call
    /// with `jwt`, and keeps the path and the `Authorization` of each call.
    async fn bots() -> (BotsClient, Calls, HttpServer) {
        let calls: Calls = Arc::default();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let seen = Arc::clone(&calls);
        let server = HttpServer::run(listener, move |request, _| {
            let path = request.uri().path().to_string();
            let authorization = request
                .headers()
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            seen.lock().unwrap().push((path.clone(), authorization));
            async move {
                if path.ends_with("/heartbeat") {
                    error_response(HttpStatus::GONE, "ended")
                } else {
                    json_response(HttpStatus::OK, &json!({ "token": "jwt" }))
                }
            }
        });
        (
            BotsClient::new(url, "recorder".into(), "b1".into(), Some("key".into())),
            calls,
            server,
        )
    }

    #[tokio::test]
    async fn a_bot_presents_its_key_and_stops_once_bots_takes_it_as_ended() {
        // Arrange
        let (client, calls, _server) = bots().await;

        // Act
        let token = client.token().await.unwrap();
        let stop = client.heartbeat(BotState::Running, None).await.unwrap();

        // Assert
        assert_eq!(token.as_deref(), Some("jwt"));
        assert!(stop);
        assert_eq!(
            *calls.lock().unwrap(),
            [
                (
                    "/bots/recorder/b1/token".to_string(),
                    "Bearer key".to_string()
                ),
                (
                    "/bots/recorder/b1/heartbeat".to_string(),
                    "Bearer key".to_string()
                ),
            ]
        );
    }
}
