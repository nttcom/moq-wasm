use std::{collections::VecDeque, sync::Mutex, time::Duration};

use aws_lc_rs::digest::{SHA256, digest};
use reqwest::StatusCode;
use telemetry::client_report::{ClientReport, ClientService};
use telemetry_report::random_u64;

use super::report_batches::{Batch, ReportBatches};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REJECTED_TOKENS: usize = 64;

pub(super) struct Reporter {
    http: reqwest::Client,
    endpoint: String,
    page: String,
    client_id: String,
    batches: ReportBatches,
    /// The SHA-256 digest of each token the server answered 401 to, the
    /// oldest first; digests, so no credential outlives its session.
    rejected_tokens: Mutex<VecDeque<Vec<u8>>>,
}

impl Reporter {
    pub(super) fn new(url: &str, page: String, batches: ReportBatches) -> Self {
        Self {
            http: reqwest::Client::new(),
            endpoint: format!("{}/api/client-telemetry", url.trim_end_matches('/')),
            page,
            client_id: format!("{:016x}{:016x}", random_u64(), random_u64()),
            batches,
            rejected_tokens: Mutex::default(),
        }
    }

    pub(super) async fn flush(&self) {
        let (batches, dropped) = self.batches.take();
        if dropped > 0 {
            tracing::warn!(
                dropped_records = dropped,
                "telemetry records dropped past the session token limit"
            );
        }
        for batch in batches {
            self.send(batch).await;
        }
    }

    async fn send(&self, batch: Batch) {
        let token = batch.token.filter(|token| !self.is_rejected(token));
        let report = ClientReport {
            token: token.clone(),
            service: ClientService::Rust,
            client_id: self.client_id.clone(),
            page: self.page.clone(),
            logs: batch.logs.into(),
            stats: Vec::new(),
            publisher_stats: Vec::new(),
            spans: batch.spans.into(),
        };
        let result = self
            .http
            .post(&self.endpoint)
            .timeout(REQUEST_TIMEOUT)
            .json(&report)
            .send()
            .await;
        if let Ok(response) = &result
            && response.status() == StatusCode::UNAUTHORIZED
            && let Some(token) = &token
        {
            // Sent without it from now on, so the server takes the records
            // as anonymous instead of dropping them.
            self.reject(token);
        }
        if let Err(error) = result.and_then(reqwest::Response::error_for_status) {
            tracing::debug!(%error, url = %self.endpoint, "telemetry report failed");
        }
    }

    fn is_rejected(&self, token: &str) -> bool {
        let token_digest = digest(&SHA256, token.as_bytes());
        self.rejected_tokens
            .lock()
            .unwrap()
            .iter()
            .any(|rejected| rejected.as_slice() == token_digest.as_ref())
    }

    fn reject(&self, token: &str) {
        let mut rejected = self.rejected_tokens.lock().unwrap();
        if rejected.len() == MAX_REJECTED_TOKENS {
            rejected.pop_front();
        }
        rejected.push_back(digest(&SHA256, token.as_bytes()).as_ref().to_vec());
    }
}
