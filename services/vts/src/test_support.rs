use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use app_registry::Apps;
use auth_token::sign_token;
use serde_json::{Value, json};
use tracing::{
    Subscriber,
    field::{Field, Visit},
    span::{Id, Record},
    subscriber::DefaultGuard,
};
use tracing_subscriber::{Layer, layer::Context, prelude::*, registry::LookupSpan};

pub(crate) const APP_ID: &str = "APP";
pub(crate) const APP_SECRET: &str = "app-secret";
pub(crate) const RELAY_ID: &str = "RELAY";
pub(crate) const RELAY_SECRET: &str = "relay-secret";
pub(crate) const NOW: u64 = 1_759_600_000;
pub(crate) const TEMPORARY_APP_ID: &str = "temp-0123456789abcdef";
pub(crate) const TEMPORARY_SECRET: &str = "temporary-secret";

pub(crate) fn test_apps_json() -> String {
    json!([
        { "appId": APP_ID, "secret": APP_SECRET, "isRelay": false },
        { "appId": RELAY_ID, "secret": RELAY_SECRET, "isRelay": true },
    ])
    .to_string()
}

pub(crate) fn test_apps() -> Apps {
    Apps::parse(&test_apps_json()).unwrap()
}

pub(crate) fn app_claims() -> Value {
    json!({
        "appId": APP_ID,
        "publish": "site1",
        "subscribe": "site1",
        "iat": NOW,
        "exp": NOW + 3600,
    })
}

pub(crate) fn temporary_app_claims() -> Value {
    json!({
        "appId": TEMPORARY_APP_ID,
        "publish": "",
        "subscribe": "",
        "iat": NOW,
        "exp": NOW + 3600,
    })
}

pub(crate) fn token(claims: &Value, secret: &str) -> String {
    sign_token(claims, secret).unwrap()
}

/// The values recorded on spans after their creation, by span name.
#[derive(Clone, Default)]
pub(crate) struct RecordedFields(Arc<Mutex<HashMap<&'static str, SpanFields>>>);

pub(crate) type SpanFields = HashMap<&'static str, String>;

impl RecordedFields {
    pub(crate) fn of(&self, span_name: &str) -> SpanFields {
        self.0
            .lock()
            .unwrap()
            .get(span_name)
            .cloned()
            .unwrap_or_default()
    }
}

/// Captures the server's spawned tasks too, but only on a current-thread
/// runtime.
pub(crate) fn capture_recorded_fields() -> (RecordedFields, DefaultGuard) {
    let fields = RecordedFields::default();
    let subscriber = tracing_subscriber::registry().with(fields.clone());
    (fields, telemetry::testing::set_default(subscriber))
}

struct FieldVisitor<'a>(&'a mut SpanFields);

impl Visit for FieldVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name(), value.to_string());
    }
}

impl<S> Layer<S> for RecordedFields
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut spans = self.0.lock().unwrap();
            values.record(&mut FieldVisitor(spans.entry(span.name()).or_default()));
        }
    }
}
