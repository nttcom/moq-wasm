use std::sync::OnceLock;

use crate::modules::moqt::version::ControlChannel;

use crate::modules::moqt::control_plane::control_messages::messages::parameters::trace_context::TraceContext;

static TRACE_CONTEXT_PROVIDER: OnceLock<fn() -> Option<String>> = OnceLock::new();

/// Sets the function that returns the W3C `traceparent` of the current span.
/// SUBSCRIBE, PUBLISH, FETCH, SUBSCRIBE_NAMESPACE and PUBLISH_NAMESPACE carry
/// its value as TRACE_CONTEXT, read inside the request's span, and
/// CLIENT_SETUP carries the value read inside `moqt.session.connect`. Only
/// the first call takes effect.
pub fn set_trace_context_provider(provider: fn() -> Option<String>) {
    let _ = TRACE_CONTEXT_PROVIDER.set(provider);
}

/// `None` unless the peer's SETUP declared TRACE_CONTEXT, a provider is set
/// and its value is a traceparent.
pub(crate) fn current_trace_context(control: &ControlChannel) -> Option<TraceContext> {
    if !control.peer_reads_trace_context() {
        return None;
    }
    current_traceparent()
}

/// `false` only in a test that called
/// `test_support::without_trace_context_declaration`.
#[cfg(not(all(test, not(target_arch = "wasm32"))))]
pub(crate) fn server_declares_trace_context() -> bool {
    true
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn server_declares_trace_context() -> bool {
    crate::modules::test_support::server_declares_trace_context()
}

pub(crate) fn current_traceparent() -> Option<TraceContext> {
    let traceparent = TRACE_CONTEXT_PROVIDER.get()?()?;
    TraceContext::parse(traceparent.as_bytes())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::cell::Cell;

    use super::set_trace_context_provider;
    use crate::SessionEvent;
    use crate::TraceContext;
    use crate::modules::moqt::control_plane::control_messages::messages::parameters::trace_context::TEST_TRACEPARENT;
    use crate::modules::test_support::{
        native_client, send_without_waiting, spawn_connected_native_sessions,
        spawn_native_server_handshake, within, without_trace_context_declaration,
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
    async fn client_setup_carries_the_trace_context() {
        // Arrange
        set_trace_context_provider(test_provider);
        PROVIDES_TRACE_CONTEXT.set(true);
        let (port, accept) = spawn_native_server_handshake("trace-context-client-setup");
        let url = format!("moqt://127.0.0.1:{port}");

        // Act
        let _client = tokio::spawn(async move { native_client().connect(&url).await?.await });
        let handshake = within(accept).await.unwrap();

        // Assert
        assert_eq!(
            handshake.trace_context().map(TraceContext::as_str),
            Some(TEST_TRACEPARENT)
        );
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
