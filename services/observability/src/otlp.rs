use std::collections::BTreeMap;

use anyhow::{Context, bail};
use serde_json::{Value, json};

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Gauge,
    /// A total since the measured session or player started.
    Counter,
}

pub(crate) fn string_attribute(key: &str, value: impl Into<Value>) -> Value {
    json!({ "key": key, "value": { "stringValue": value.into() } })
}

/// OTLP's `timeUnixNano` for a Unix time in milliseconds.
pub(crate) fn time_unix_nano(unix_ms: u64) -> String {
    (u128::from(unix_ms) * 1_000_000).to_string()
}

/// OTLP data points grouped into one `Metric` per name.
#[derive(Default)]
pub(crate) struct Metrics(BTreeMap<&'static str, (&'static str, Kind, Vec<Value>)>);

impl Metrics {
    /// `point` is an OTLP `NumberDataPoint`; the first point of a name fixes its unit and kind.
    pub(crate) fn add(&mut self, name: &'static str, unit: &'static str, kind: Kind, point: Value) {
        self.0
            .entry(name)
            .or_insert_with(|| (unit, kind, Vec::new()))
            .2
            .push(point);
    }

    pub(crate) fn into_otlp(self) -> Vec<Value> {
        self.0
            .into_iter()
            .map(|(name, (unit, kind, data_points))| match kind {
                Kind::Gauge => json!({
                    "name": name,
                    "unit": unit,
                    "gauge": { "dataPoints": data_points },
                }),
                Kind::Counter => json!({
                    "name": name,
                    "unit": unit,
                    "sum": {
                        // OTLP AggregationTemporality 2 is CUMULATIVE.
                        "aggregationTemporality": 2,
                        "isMonotonic": true,
                        "dataPoints": data_points,
                    },
                }),
            })
            .collect()
    }
}

pub(crate) async fn post(
    http: &reqwest::Client,
    endpoint: &str,
    body: String,
) -> anyhow::Result<()> {
    let response = http
        .post(endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .with_context(|| format!("the OTLP endpoint {endpoint} is unreachable"))?;
    let status = response.status();
    if !status.is_success() {
        bail!(
            "the OTLP endpoint {endpoint} answered {status}: {}",
            response.text().await?.trim()
        );
    }
    Ok(())
}
