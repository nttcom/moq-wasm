use std::sync::{Arc, Mutex};

use tracing::{Subscriber, span::Id};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

#[derive(Clone, Default)]
pub(crate) struct ClosedSpans(Arc<Mutex<Vec<&'static str>>>);

impl ClosedSpans {
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.0.lock().unwrap().contains(&name)
    }

    pub(crate) fn names(&self) -> Vec<&'static str> {
        self.0.lock().unwrap().clone()
    }
}

impl<S> Layer<S> for ClosedSpans
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(&id) {
            self.0.lock().unwrap().push(span.name());
        }
    }
}
