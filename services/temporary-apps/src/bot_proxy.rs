use std::{net::IpAddr, time::Instant};

use anyhow::{Context, Result};
use hyper::{Method, StatusCode, body::Bytes};
use serde_json::Value;

use crate::rate_limit::RateLimit;

/// Calls `bot-dispatcher` for the pages of temporary apps, as an app server calls it for
/// its own pages (docs/architecture/bot-dispatcher/architecture.md "Temporary apps").
pub struct BotProxy {
    client: reqwest::Client,
    bot_dispatcher_url: String,
    types: Vec<String>,
    launch_limit: RateLimit,
}

impl BotProxy {
    pub fn new(bot_dispatcher_url: String, types: Vec<String>, launch_limit: RateLimit) -> Self {
        Self {
            client: reqwest::Client::new(),
            bot_dispatcher_url: bot_dispatcher_url.trim_end_matches('/').to_string(),
            types,
            launch_limit,
        }
    }

    pub(crate) fn allows(&self, bot_type: &str) -> bool {
        self.types.iter().any(|allowed| allowed == bot_type)
    }

    pub(crate) fn allow_launch(&self, client: IpAddr, now: Instant) -> bool {
        self.launch_limit.allow(client, now)
    }

    /// Sends the request to the same path of `bot-dispatcher` with `launch_token` and
    /// returns its status and body unchanged. Fails only when `bot-dispatcher` cannot
    /// be reached.
    pub(crate) async fn forward(
        &self,
        method: Method,
        path_and_query: &str,
        launch_token: &str,
        body: Option<Value>,
    ) -> Result<(StatusCode, Bytes)> {
        let url = format!("{}{path_and_query}", self.bot_dispatcher_url);
        let mut request = self.client.request(method, &url).bearer_auth(launch_token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("cannot reach {url}"))?;
        Ok((response.status(), response.bytes().await?))
    }
}
