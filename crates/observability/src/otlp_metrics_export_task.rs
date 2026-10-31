use std::time::Duration;

use tokio::{task::JoinHandle, time};

use crate::{latest_snapshots::LatestSnapshots, otlp, otlp_metrics::export_request};

const EXPORT_INTERVAL: Duration = Duration::from_secs(10);

pub struct OtlpMetricsExportTask {
    join_handle: JoinHandle<()>,
}

impl OtlpMetricsExportTask {
    pub fn run(latest: LatestSnapshots, endpoint: String) -> Self {
        let join_handle = tokio::spawn(async move {
            let http = reqwest::Client::new();
            let mut ticker = time::interval(EXPORT_INTERVAL);
            loop {
                ticker.tick().await;
                let snapshots = latest.current();
                if snapshots.is_empty() {
                    continue;
                }
                let body = export_request(&snapshots).to_string();
                if let Err(error) = otlp::post(&http, &endpoint, body).await {
                    tracing::warn!(%error, "relay metrics were not exported");
                }
            }
        });
        Self { join_handle }
    }
}

impl Drop for OtlpMetricsExportTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use tokio::time;

    use super::{EXPORT_INTERVAL, OtlpMetricsExportTask};
    use crate::{
        latest_snapshots::LatestSnapshots,
        test_support::{fake_http_server, snapshot},
    };

    #[tokio::test(start_paused = true)]
    async fn the_newest_snapshots_are_posted_once_per_interval() {
        // Arrange
        let (url, mut request_receiver) = fake_http_server().await;
        let latest = LatestSnapshots::default();
        let _export = OtlpMetricsExportTask::run(latest.clone(), format!("{url}/v1/metrics"));

        // Act
        time::advance(EXPORT_INTERVAL).await;

        // Assert: nothing is posted while no relay has sent a snapshot
        assert!(request_receiver.try_recv().is_err());

        // Act
        latest.update(snapshot(1_000));
        time::advance(EXPORT_INTERVAL).await;
        let (uri, body) = request_receiver.recv().await.unwrap();

        // Assert
        assert_eq!(uri, "/v1/metrics");
        assert!(body.contains("\"relay-a\""), "{body}");
    }
}
