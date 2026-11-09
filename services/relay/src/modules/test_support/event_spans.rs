use std::sync::{Arc, Mutex};

use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

/// An event's message and the name of the span it was written in.
type LoggedEvent = (String, Option<&'static str>);

#[derive(Clone, Default)]
pub(crate) struct EventSpans(Arc<Mutex<Vec<LoggedEvent>>>);

impl EventSpans {
    pub(crate) fn spans_of(&self, message: &str) -> Vec<Option<&'static str>> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(logged, _)| logged == message)
            .map(|(_, span)| *span)
            .collect()
    }
}

struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl<S> Layer<S> for EventSpans
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut message = Message(String::new());
        event.record(&mut message);
        let span = ctx.event_span(event).map(|span| span.name());
        self.0.lock().unwrap().push((message.0, span));
    }
}
