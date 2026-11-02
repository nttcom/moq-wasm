use std::time::{Duration, SystemTime, UNIX_EPOCH};

use telemetry::EventLogger;
use tokio::{task::JoinHandle, time::MissedTickBehavior};

use crate::modules::observability::{process_memory, stats_collector::StatsCollector};

const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(1);

pub struct StatsExportTask {
    join_handle: JoinHandle<()>,
}

pub(crate) fn unix_millis_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

impl StatsExportTask {
    /// Sends one snapshot per second as an OTLP log record.
    pub(crate) fn run(collector: StatsCollector, event_logger: EventLogger) -> Self {
        let join_handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(SNAPSHOT_INTERVAL);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let timestamp_ms = unix_millis_now();
                let snapshot = collector
                    .collect(timestamp_ms, process_memory::resident_set_bytes().await)
                    .await;
                let json = String::from_utf8(snapshot.to_json()).expect("serde_json writes UTF-8");
                event_logger.emit(
                    relay_stats::EVENT_NAME,
                    UNIX_EPOCH + Duration::from_millis(timestamp_ms),
                    relay_stats::EVENT_ATTRIBUTE,
                    json,
                );
            }
        });
        Self { join_handle }
    }
}

impl Drop for StatsExportTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
