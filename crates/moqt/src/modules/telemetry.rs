use std::sync::OnceLock;

use crate::modules::moqt::version::ControlChannel;

use crate::modules::moqt::control_plane::control_messages::messages::parameters::trace_context::TraceContext;

static TRACE_CONTEXT_PROVIDER: OnceLock<fn() -> Option<String>> = OnceLock::new();

/// Sets the function that returns the W3C `traceparent` of the current span.
/// SUBSCRIBE, PUBLISH, FETCH, SUBSCRIBE_NAMESPACE and PUBLISH_NAMESPACE carry
/// its value as TRACE_CONTEXT, read inside the request's span. Only the first
/// call takes effect.
pub fn set_trace_context_provider(provider: fn() -> Option<String>) {
    let _ = TRACE_CONTEXT_PROVIDER.set(provider);
}

/// `None` unless the peer's SETUP declared TRACE_CONTEXT, a provider is set
/// and its value is a traceparent.
pub(crate) fn current_trace_context(control: &ControlChannel) -> Option<TraceContext> {
    if !control.peer_reads_trace_context() {
        return None;
    }
    let traceparent = TRACE_CONTEXT_PROVIDER.get()?()?;
    TraceContext::parse(traceparent.as_bytes())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::cell::Cell;

    use super::set_trace_context_provider;
    use crate::SessionEvent;
    use crate::modules::moqt::control_plane::control_messages::messages::parameters::trace_context::TEST_TRACEPARENT;
    use crate::modules::test_support::{
        send_without_waiting, spawn_connected_native_sessions, without_trace_context_declaration,
    };
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

    async fn subscribe_trace_context(name: &str) -> Option<String> {
        set_trace_context_provider(test_provider);
        PROVIDES_TRACE_CONTEXT.set(true);
        let (client, server) = spawn_connected_native_sessions(name).await;
        let mut subscriber = client.subscriber();
        let mut request = Box::pin(subscriber.subscribe(
            "ns".to_string(),
            "track".to_string(),
            SubscribeOption::default(),
        ));
        let event = send_without_waiting(&mut request, &server).await;
        let SessionEvent::Subscribe(handler) = event else {
            panic!("expected SUBSCRIBE, got {event:?}");
        };
        handler
            .trace_context()
            .map(|context| context.as_str().to_string())
    }

    #[tokio::test]
    async fn a_subscribe_to_a_peer_that_declared_trace_context_carries_it() {
        // Act
        let trace_context = subscribe_trace_context("trace-context-declared").await;

        // Assert
        assert_eq!(trace_context.as_deref(), Some(TEST_TRACEPARENT));
    }

    #[tokio::test]
    async fn a_subscribe_to_a_peer_that_did_not_declare_trace_context_omits_it() {
        // Arrange
        without_trace_context_declaration();

        // Act
        let trace_context = subscribe_trace_context("trace-context-undeclared").await;

        // Assert
        assert_eq!(trace_context, None);
    }
}
