use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;
use tokio::{sync::oneshot, task::JoinHandle};
use url::Url;

use super::LogBuffer;
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
    pub fn run(target: ReportTarget, buffer: LogBuffer) -> Self {
        let (finish_sender, finish_receiver) = oneshot::channel();
        let join_handle = tokio::spawn(report_loop(target, buffer, finish_receiver));
        Self {
            join_handle,
            finish_sender,
        }
    }

    /// Sends the logs not reported yet and waits for the request to end.
    pub async fn finish(self) {
        let _ = self.finish_sender.send(());
        let _ = self.join_handle.await;
    }
}

async fn report_loop(
    target: ReportTarget,
    buffer: LogBuffer,
    mut finish_receiver: oneshot::Receiver<()>,
) {
    let reporter = Reporter::new(target);
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
        }
    }

    /// A failed report's logs are dropped, not retried.
    async fn send(&self, buffer: &LogBuffer) {
        let logs = buffer.take();
        if logs.is_empty() {
            return;
        }
        let token = current_auth_token(&self.target.relay).await.ok().flatten();
        let body = json!({
            "service": SERVICE,
            "token": token,
            "clientId": self.client_id,
            "page": self.target.page,
            "logs": logs,
        });
        let result = self
            .http
            .post(self.target.endpoint.clone())
            .body(body.to_string())
            .send()
            .await
            .and_then(reqwest::Response::error_for_status);
        if let Err(error) = result {
            tracing::debug!(%error, url = %self.target.endpoint, "telemetry report failed");
        }
    }
}
