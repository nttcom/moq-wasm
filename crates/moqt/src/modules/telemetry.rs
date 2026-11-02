use std::sync::OnceLock;

use crate::modules::moqt::control_plane::control_messages::messages::parameters::trace_context::TraceContext;

static TRACE_CONTEXT_PROVIDER: OnceLock<fn() -> Option<String>> = OnceLock::new();

/// Sets the function that returns the W3C `traceparent` of the current span.
/// SUBSCRIBE, PUBLISH, FETCH, SUBSCRIBE_NAMESPACE and PUBLISH_NAMESPACE carry
/// its value as TRACE_CONTEXT, read inside the request's span. Only the first
/// call takes effect.
pub fn set_trace_context_provider(provider: fn() -> Option<String>) {
    let _ = TRACE_CONTEXT_PROVIDER.set(provider);
}

/// `None` without a provider or when its value is not a traceparent.
pub(crate) fn current_trace_context() -> Option<TraceContext> {
    let traceparent = TRACE_CONTEXT_PROVIDER.get()?()?;
    TraceContext::parse(traceparent.as_bytes())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::cell::Cell;

    use super::set_trace_context_provider;
    use crate::SessionEvent;
    use crate::modules::moqt::control_plane::control_messages::messages::parameters::trace_context::TEST_TRACEPARENT;
    use crate::modules::test_support::{send_without_waiting, spawn_connected_native_sessions};
    use crate::subscribe::SubscribeOption;

    thread_local! {
        static PROVIDES_TRACE_CONTEXT: Cell<bool> = const { Cell::new(false) };
    }

    /// The provider is process-global and set once, so it answers only on
    /// the thread of the test that opts in.
    fn test_provider() -> Option<String> {
        PROVIDES_TRACE_CONTEXT
            .get()
            .then(|| TEST_TRACEPARENT.to_string())
    }

    #[tokio::test]
    async fn a_subscribe_carries_the_provider_s_trace_context() {
        // Arrange
        set_trace_context_provider(test_provider);
        PROVIDES_TRACE_CONTEXT.set(true);
        let (client, server) = spawn_connected_native_sessions("trace-context-provider").await;
        let mut subscriber = client.subscriber();

        // Act
        let mut request = Box::pin(subscriber.subscribe(
            "ns".to_string(),
            "track".to_string(),
            SubscribeOption::default(),
        ));
        let event = send_without_waiting(&mut request, &server).await;

        // Assert
        let SessionEvent::Subscribe(handler) = event else {
            panic!("expected SUBSCRIBE, got {event:?}");
        };
        assert_eq!(
            handler.trace_context().map(|context| context.as_str()),
            Some(TEST_TRACEPARENT)
        );
    }
}
