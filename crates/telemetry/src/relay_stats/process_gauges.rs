use opentelemetry::metrics::{Gauge, Meter};

use super::ProcessStats;

/// The relay-wide values of a snapshot as OTLP gauges, so HyperDX can chart
/// and alert on them.
pub struct ProcessGauges {
    rss: Gauge<u64>,
    cache_tracks: Gauge<u64>,
    cache_objects: Gauge<u64>,
    cache_payload: Gauge<u64>,
    disk_cache_payload: Gauge<u64>,
    disk_cache_max: Gauge<u64>,
    client_sessions: Gauge<u64>,
    draining: Gauge<u64>,
}

impl ProcessGauges {
    pub fn new(meter: &Meter) -> Self {
        let gauge =
            |name: &'static str, unit: &'static str| meter.u64_gauge(name).with_unit(unit).build();
        Self {
            rss: gauge("relay.process.rss", "By"),
            cache_tracks: gauge("relay.cache.tracks", "{track}"),
            cache_objects: gauge("relay.cache.objects", "{object}"),
            cache_payload: gauge("relay.cache.payload", "By"),
            disk_cache_payload: gauge("relay.disk_cache.payload", "By"),
            disk_cache_max: gauge("relay.disk_cache.max", "By"),
            client_sessions: gauge("relay.client_sessions", "{session}"),
            draining: gauge("relay.draining", "1"),
        }
    }

    pub fn record(&self, process: &ProcessStats) {
        if let Some(rss_bytes) = process.rss_bytes {
            self.rss.record(rss_bytes, &[]);
        }
        self.cache_tracks.record(process.cache_tracks, &[]);
        self.cache_objects.record(process.cache_objects, &[]);
        self.cache_payload.record(process.cache_payload_bytes, &[]);
        self.disk_cache_payload
            .record(process.disk_cache_bytes, &[]);
        self.disk_cache_max
            .record(process.disk_cache_max_bytes, &[]);
        self.client_sessions.record(process.client_sessions, &[]);
        self.draining
            .record(u64::from(process.draining_since_ms.is_some()), &[]);
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_sdk::metrics::{
        InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
        data::{AggregatedMetrics, MetricData},
    };

    use super::ProcessGauges;
    use crate::relay_stats::ProcessStats;

    fn last_value(exporter: &InMemoryMetricExporter, name: &str) -> Option<u64> {
        let exported = exporter.get_finished_metrics().unwrap();
        let metric = exported
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
            .find(|metric| metric.name() == name)?;
        match metric.data() {
            AggregatedMetrics::U64(MetricData::Gauge(gauge)) => {
                gauge.data_points().last().map(|point| point.value())
            }
            _ => None,
        }
    }

    #[test]
    fn a_snapshot_s_process_stats_become_gauges() {
        // Arrange
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        let gauges = ProcessGauges::new(&provider.meter("relay"));
        let process = ProcessStats {
            rss_bytes: None,
            cache_tracks: 3,
            client_sessions: 2,
            draining_since_ms: Some(1),
            ..ProcessStats::default()
        };

        // Act
        gauges.record(&process);
        provider.force_flush().unwrap();

        // Assert: an unknown RSS sends no point rather than 0
        assert_eq!(last_value(&exporter, "relay.cache.tracks"), Some(3));
        assert_eq!(last_value(&exporter, "relay.client_sessions"), Some(2));
        assert_eq!(last_value(&exporter, "relay.draining"), Some(1));
        assert_eq!(last_value(&exporter, "relay.process.rss"), None);
    }
}
