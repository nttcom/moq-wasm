use std::{sync::OnceLock, time::Duration};

use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use serde_json::{Value, json};

/// The environment variable `bot-dispatcher` passes a locally launched bot its key in.
const BOT_KEY_ENV: &str = "BOT_DISPATCHER_BOT_KEY";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// What a bot reports to `bot-dispatcher` at every heartbeat.
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

/// The calls a recording started by `bot-dispatcher` makes back to it
/// (docs/architecture/bot-dispatcher/architecture.md): its token, and its heartbeat.
/// It authenticates with the key `bot-dispatcher` gave it in `BOT_DISPATCHER_BOT_KEY` when it was
/// launched locally, and otherwise with the Google ID token of the job's
/// service account for the bot-dispatcher URL.
pub(crate) struct BotDispatcherClient {
    http: reqwest::Client,
    url: String,
    bot_type: String,
    id: String,
    key: Option<String>,
    /// Sent with every heartbeat once set.
    output: OnceLock<Value>,
}

impl BotDispatcherClient {
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
            output: OnceLock::new(),
        }
    }

    /// The type `bot-dispatcher` started the bot as, the last element of the namespace
    /// its token may publish: `<room>/bots/<type>`.
    pub(crate) fn bot_type(&self) -> &str {
        &self.bot_type
    }

    /// A new token for the relay; `None` from a `bot-dispatcher` without authorization,
    /// whose bots join with no token. Fails when `bot-dispatcher` refuses, as it does
    /// once the bot has ended.
    pub(crate) async fn token(&self) -> Result<Option<String>> {
        let (status, body) = self.post("token", None).await?;
        if !status.is_success() {
            bail!("bot-dispatcher answered {status}: {}", body["error"]);
        }
        match &body["token"] {
            Value::String(token) => Ok(Some(token.clone())),
            Value::Null => Ok(None),
            _ => bail!("bot-dispatcher answered without a token"),
        }
    }

    /// Makes every later heartbeat report `recording_id` as what the bot
    /// produces; a later call is ignored.
    pub(crate) fn set_recording_id(&self, recording_id: &str) {
        let _ = self.output.set(json!({ "recording_id": recording_id }));
    }

    /// `true` when `bot-dispatcher` asks the bot to stop, or takes it as ended.
    pub(crate) async fn heartbeat(&self, state: BotState, error: Option<&str>) -> Result<bool> {
        let mut body = json!({ "state": state.as_str(), "error": error });
        if let Some(output) = self.output.get() {
            body["output"] = output.clone();
        }
        let (status, body) = self.post("heartbeat", Some(body)).await?;
        if status == StatusCode::GONE {
            return Ok(true);
        }
        if !status.is_success() {
            bail!("bot-dispatcher answered {status}: {}", body["error"]);
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
            .headers(telemetry_otlp::current_trace_headers())
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

    use http_server::{HttpServer, error_response, json_response, read_json};
    use hyper::{StatusCode as HttpStatus, header::AUTHORIZATION};
    use tokio::net::TcpListener;

    use super::*;

    type Calls = Arc<Mutex<Vec<(String, String, Value)>>>;

    /// A `bot-dispatcher` that answers every heartbeat `410 Gone` and every token call
    /// with `jwt`, and keeps the path, the `Authorization` and the body
    /// (`Null` when none) of each call.
    async fn bot_dispatcher() -> (BotDispatcherClient, Calls, HttpServer) {
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
            let seen = Arc::clone(&seen);
            async move {
                let body = read_json::<Value, _>(request.into_body())
                    .await
                    .unwrap_or_default();
                seen.lock()
                    .unwrap()
                    .push((path.clone(), authorization, body));
                if path.ends_with("/heartbeat") {
                    error_response(HttpStatus::GONE, "ended")
                } else {
                    json_response(HttpStatus::OK, &json!({ "token": "jwt" }))
                }
            }
        });
        (
            BotDispatcherClient::new(url, "recorder".into(), "b1".into(), Some("key".into())),
            calls,
            server,
        )
    }

    #[tokio::test]
    async fn a_bot_presents_its_key_and_stops_once_bot_dispatcher_takes_it_as_ended() {
        // Arrange
        let (client, calls, _server) = bot_dispatcher().await;

        // Act
        let token = client.token().await.unwrap();
        let stop = client.heartbeat(BotState::Running, None).await.unwrap();

        // Assert
        assert_eq!(token.as_deref(), Some("jwt"));
        assert!(stop);
        let calls = calls.lock().unwrap();
        let seen: Vec<_> = calls
            .iter()
            .map(|(path, authorization, _)| (path.as_str(), authorization.as_str()))
            .collect();
        assert_eq!(
            seen,
            [
                ("/bots/recorder/b1/token", "Bearer key"),
                ("/bots/recorder/b1/heartbeat", "Bearer key"),
            ]
        );
    }

    #[tokio::test]
    async fn heartbeats_report_the_recording_once_it_exists() {
        // Arrange
        let (client, calls, _server) = bot_dispatcher().await;

        // Act
        client
            .heartbeat(BotState::Failed, Some("early"))
            .await
            .unwrap();
        client.set_recording_id("r1");
        client.heartbeat(BotState::Running, None).await.unwrap();

        // Assert
        let calls = calls.lock().unwrap();
        assert_eq!(calls[0].2, json!({ "state": "failed", "error": "early" }));
        assert_eq!(
            calls[1].2,
            json!({ "state": "running", "error": null, "output": { "recording_id": "r1" } })
        );
    }
}
