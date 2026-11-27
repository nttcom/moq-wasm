//! Sends the app's `tracing` logs and spans to the observability server's
//! `/api/client-telemetry`, as the browser and Python SDKs' `telemetry`
//! option does. Records under a session carry the session's latest token, so
//! the server files them under its app; others go untokenized.

mod flush_task;
mod report_batches;
mod reporter;
mod session_scope;

use std::sync::Arc;

use telemetry_report::{
    ClientSpan, SpanRecordLayer, continue_trace_from_traceparent, current_scope,
    current_traceparent, log_record, now_ms, random_u64,
};
use tracing::{Event, Subscriber, level_filters::LevelFilter};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

use flush_task::FlushTask;
use report_batches::ReportBatches;
use reporter::Reporter;
pub(crate) use session_scope::SessionScope;
use session_scope::{carry_scope, token_of};

#[derive(Debug, Clone)]
pub struct TelemetryOptions {
    /// The observability server's origin (`https://observability.example`);
    /// reports go to its `/api/client-telemetry`.
    pub url: String,
    /// Names the app in each report; the server rejects every report when it
    /// is empty or over 128 characters.
    pub page: String,
}

/// Owns the task that posts the queued records every 10 s; dropping it stops
/// the task, so await `flush` before the app exits.
pub struct Telemetry {
    reporter: Arc<Reporter>,
    _flush_task: FlushTask,
}

impl Telemetry {
    /// Posts the records queued so far, one report per token, each waiting
    /// 5 s at most. A failed report's records are dropped, not retried.
    pub async fn flush(&self) {
        self.reporter.flush().await;
    }
}

/// Returns a layer that queues each event as a log and each closed INFO or
/// higher span, and the `Telemetry` that posts them. Add it to your
/// `tracing_subscriber::registry()`, filtered to the levels to send, before
/// `connect`: records of the session's `moqt` tasks and of futures passed to
/// `Session::scoped` then carry its token, and requests carry the current
/// span's trace as TRACE_CONTEXT. Panics outside a Tokio runtime.
pub fn layer<S>(options: TelemetryOptions) -> (impl Layer<S>, Telemetry)
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    moqt::executor::set_spawn_wrapper(carry_scope);
    moqt::set_trace_context_provider(current_traceparent);
    moqt::set_trace_context_joiner(continue_trace_from_traceparent);
    let batches = ReportBatches::default();
    let spans = batches.clone();
    let span_layer = SpanRecordLayer {
        now_ms,
        random_byte: || random_u64() as u8,
        emit: move |span: ClientSpan, scope: Option<u64>| spans.push_span(token_of(scope), span),
    };
    let layer = LogLayer(batches.clone()).and_then(span_layer.with_filter(LevelFilter::INFO));
    let reporter = Arc::new(Reporter::new(&options.url, options.page, batches));
    let telemetry = Telemetry {
        _flush_task: FlushTask::run(reporter.clone()),
        reporter,
    };
    (layer, telemetry)
}

struct LogLayer(ReportBatches);

impl<S: Subscriber> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        self.0
            .push_log(token_of(current_scope()), log_record(event, now_ms()));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::Value;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;
    use crate::test_support::{FakeTelemetryServer, TestRelay};

    /// The report holding the log with `message`.
    fn report_with<'a>(reports: &'a [Value], message: &str) -> &'a Value {
        reports
            .iter()
            .find(|report| {
                report["logs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|log| log["message"] == message)
            })
            .unwrap_or_else(|| panic!("no report holds {message:?}: {reports:?}"))
    }

    fn install(server: &FakeTelemetryServer) -> (tracing::subscriber::DefaultGuard, Telemetry) {
        let (layer, telemetry) = layer(TelemetryOptions {
            url: format!("{}/", server.url),
            page: "test-app".to_string(),
        });
        let subscriber = tracing_subscriber::registry().with(layer);
        (tracing::subscriber::set_default(subscriber), telemetry)
    }

    #[tokio::test]
    async fn a_log_under_a_session_is_posted_with_its_token_and_one_outside_without() {
        // Arrange
        let mut server = FakeTelemetryServer::spawn(200).await;
        let (_guard, telemetry) = install(&server);
        let relay = TestRelay::spawn("telemetry-session-token");
        let session = relay.connect_with_token("session-token").await;

        // Act
        tokio::spawn(session.scoped(async { tracing::info!("inside the session") }))
            .await
            .unwrap();
        tracing::info!("outside any session");
        telemetry.flush().await;

        // Assert: the session's connect span carries its token as well
        let reports = server.reports();
        let inside = report_with(&reports, "inside the session");
        assert_eq!(inside["token"], "session-token");
        assert_eq!(inside["service"], "rust");
        assert_eq!(inside["page"], "test-app");
        let session_spans: Vec<_> = inside["spans"]
            .as_array()
            .unwrap()
            .iter()
            .map(|span| &span["name"])
            .collect();
        assert!(
            session_spans.contains(&&Value::from("moqt.session.connect")),
            "{inside}"
        );
        assert_eq!(
            report_with(&reports, "outside any session")["token"],
            Value::Null
        );
    }

    #[tokio::test]
    async fn a_span_under_a_session_is_posted_with_its_token() {
        // Arrange
        let mut server = FakeTelemetryServer::spawn(200).await;
        let (_guard, telemetry) = install(&server);
        let token = Arc::new(Mutex::new("token".to_string()));
        let scope = SessionScope::new(Some(&token));

        // Act
        scope
            .scoped(async {
                tracing::info_span!("request")
                    .in_scope(|| tracing::debug_span!("detail").in_scope(|| {}));
            })
            .await;
        telemetry.flush().await;

        // Assert: the DEBUG span is not sent
        let reports = server.reports();
        let [report] = reports.as_slice() else {
            panic!("{reports:?}");
        };
        assert_eq!(report["token"], "token");
        let names: Vec<_> = report["spans"]
            .as_array()
            .unwrap()
            .iter()
            .map(|span| span["name"].clone())
            .collect();
        assert_eq!(names, vec!["request"]);
    }

    #[tokio::test]
    async fn a_log_after_a_token_refresh_carries_the_new_token() {
        // Arrange
        let mut server = FakeTelemetryServer::spawn(200).await;
        let (_guard, telemetry) = install(&server);
        let token = Arc::new(Mutex::new("first".to_string()));
        let scope = SessionScope::new(Some(&token));
        *token.lock().unwrap() = "second".to_string();

        // Act
        scope.scoped(async { tracing::info!("refreshed") }).await;
        telemetry.flush().await;

        // Assert
        assert_eq!(
            report_with(&server.reports(), "refreshed")["token"],
            "second"
        );
    }

    #[tokio::test]
    async fn a_token_the_server_rejected_is_left_out_of_later_reports() {
        // Arrange
        let mut server = FakeTelemetryServer::spawn(401).await;
        let (_guard, telemetry) = install(&server);
        let token = Arc::new(Mutex::new("rejected".to_string()));
        let scope = SessionScope::new(Some(&token));
        scope.scoped(async { tracing::info!("first") }).await;
        telemetry.flush().await;

        // Act
        scope.scoped(async { tracing::info!("second") }).await;
        telemetry.flush().await;

        // Assert
        let reports = server.reports();
        assert_eq!(report_with(&reports, "first")["token"], "rejected");
        assert_eq!(report_with(&reports, "second")["token"], Value::Null);
    }
}
