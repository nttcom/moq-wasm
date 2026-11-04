use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::StatusCode;
use serde_json::json;
use tokio::{sync::oneshot, task::JoinHandle};
use url::Url;

use super::ReportBuffer;
use crate::{cli::RelayArgs, transport::current_auth_token};

const REPORT_INTERVAL: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const SERVICE: &str = "moq-cli";

pub struct ReportTarget {
    pub endpoint: Url,
    /// The subcommand, sent as the report's `page`.
    pub page: &'static str,
    /// Its token, re-read before every report, ties the logs to the token's app.
    pub relay: RelayArgs,
}

pub struct ReportTask {
    join_handle: JoinHandle<()>,
    finish_sender: oneshot::Sender<()>,
}

impl ReportTask {
    pub fn run(target: ReportTarget, buffer: ReportBuffer) -> Self {
        let (finish_sender, finish_receiver) = oneshot::channel();
        let join_handle = tokio::spawn(report_loop(target, buffer, finish_receiver));
        Self {
            join_handle,
            finish_sender,
        }
    }

    /// Sends the logs and spans not reported yet and waits for the request to end.
    pub async fn finish(self) {
        let _ = self.finish_sender.send(());
        let _ = self.join_handle.await;
    }
}

async fn report_loop(
    target: ReportTarget,
    buffer: ReportBuffer,
    mut finish_receiver: oneshot::Receiver<()>,
) {
    let mut reporter = Reporter::new(target);
    let mut interval = tokio::time::interval_at(
        tokio::time::Instant::now() + REPORT_INTERVAL,
        REPORT_INTERVAL,
    );
    loop {
        tokio::select! {
            _ = interval.tick() => reporter.send(&buffer).await,
            _ = &mut finish_receiver => {
                reporter.send(&buffer).await;
                return;
            }
        }
    }
}

struct Reporter {
    http: reqwest::Client,
    client_id: String,
    target: ReportTarget,
    /// The token the server last answered 401 to. Reports leave it out, so the
    /// server takes them as anonymous instead of dropping them.
    rejected_token: Option<String>,
}

impl Reporter {
    fn new(target: ReportTarget) -> Self {
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .unwrap_or_default(),
            client_id: format!("{started:x}-{}", std::process::id()),
            target,
            rejected_token: None,
        }
    }

    /// A failed report's logs and spans are dropped, not retried.
    async fn send(&mut self, buffer: &ReportBuffer) {
        let (logs, spans) = buffer.take();
        if logs.is_empty() && spans.is_empty() {
            return;
        }
        let token = current_auth_token(&self.target.relay)
            .await
            .ok()
            .flatten()
            .filter(|token| self.rejected_token.as_ref() != Some(token));
        let body = json!({
            "service": SERVICE,
            "token": token,
            "clientId": self.client_id,
            "page": self.target.page,
            "logs": logs,
            "spans": spans,
        });
        let result = self
            .http
            .post(self.target.endpoint.clone())
            .body(body.to_string())
            .send()
            .await;
        if let Ok(response) = &result
            && response.status() == StatusCode::UNAUTHORIZED
            && token.is_some()
        {
            self.rejected_token = token;
        }
        if let Err(error) = result.and_then(reqwest::Response::error_for_status) {
            tracing::debug!(%error, url = %self.target.endpoint, "telemetry report failed");
        }
    }
}
