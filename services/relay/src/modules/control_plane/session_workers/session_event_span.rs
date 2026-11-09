use moqt::TraceContext;
use tracing::Span;

use crate::logging::{link_span, relay_hostname};
use crate::modules::{
    auth::verified_token::VerifiedToken, domain::session_id::SessionId,
    session::moqt_session_event::MoqtSessionEvent,
};

/// A request's span roots its own trace, or continues the client's when the
/// request carries TRACE_CONTEXT, and links to the session span. Only the
/// lifecycle events `Disconnected` and `ProtocolViolation` are children of
/// the session span.
pub(super) fn session_event_span(
    session_id: SessionId,
    session_span: &Span,
    token: Option<&VerifiedToken>,
    event: &MoqtSessionEvent,
) -> Span {
    let app_id = token.map(|token| token.app_id.as_str());
    let peer_relay_id = token.and_then(|token| token.relay_id.as_deref());
    let span = match event {
        MoqtSessionEvent::PublishNamespace(handler) => tracing::info_span!(
            parent: None,
            "relay.publish_namespace",
            session_id = %session_id,
            app_id,
            relay_hostname = relay_hostname(),
            track_namespace = %handler.track_namespace(),
        ),
        MoqtSessionEvent::PublishNamespaceDone(handler) => tracing::info_span!(
            parent: None,
            "relay.publish_namespace_done",
            session_id = %session_id,
            app_id,
            relay_hostname = relay_hostname(),
            track_namespace = %handler.track_namespace(),
        ),
        MoqtSessionEvent::SubscribeNamespace(handler) => tracing::info_span!(
            parent: None,
            "relay.subscribe_namespace",
            session_id = %session_id,
            app_id,
            relay_hostname = relay_hostname(),
            track_namespace_prefix = %handler.track_namespace_prefix(),
        ),
        MoqtSessionEvent::SubscribeNamespaceUpdate(handler) => tracing::info_span!(
            parent: session_span,
            "relay.session.event",
            session_id = %session_id,
            event = "SubscribeNamespaceUpdate",
            track_namespace_prefix = %handler.track_namespace_prefix(),
            new_track_namespace_prefix = %handler.new_track_namespace_prefix(),
            interest = ?handler.interest(),
        ),
        MoqtSessionEvent::UnsubscribeNamespace(handler) => tracing::info_span!(
            parent: None,
            "relay.unsubscribe_namespace",
            session_id = %session_id,
            app_id,
            relay_hostname = relay_hostname(),
            track_namespace_prefix = %handler.track_namespace_prefix(),
        ),
        MoqtSessionEvent::PublishBlocked(handler) => tracing::info_span!(
            parent: session_span,
            "relay.session.event",
            session_id = %session_id,
            event = "PublishBlocked",
            track_namespace = %handler.track_namespace,
            track_name = %handler.track_name,
        ),
        MoqtSessionEvent::Publish(handler) => tracing::info_span!(
            parent: None,
            "relay.publish",
            session_id = %session_id,
            app_id,
            relay_hostname = relay_hostname(),
            track_namespace = %handler.track_namespace(),
            track_name = %handler.track_name(),
            track_alias = handler.track_alias(),
            group_order = ?handler._group_order(),
            content_exists = ?handler._content_exists(),
            forward = handler._forward(),
        ),
        MoqtSessionEvent::Subscribe(handler) => tracing::info_span!(
            parent: None,
            "relay.subscribe",
            session_id = %session_id,
            app_id,
            peer_relay_id,
            upstream_relay_id = tracing::field::Empty,
            upstream_request_id = tracing::field::Empty,
            relay_hostname = relay_hostname(),
            request_id = handler.subscribe_id(),
            track_namespace = %handler.track_namespace(),
            track_name = %handler.track_name(),
            subscriber_priority = handler._subscriber_priority(),
            group_order = ?handler._group_order(),
            forward = handler.forward(),
            filter_type = ?handler._filter_type(),
        ),
        MoqtSessionEvent::Unsubscribe(handler) => tracing::info_span!(
            parent: None,
            "relay.unsubscribe",
            session_id = %session_id,
            app_id,
            relay_hostname = relay_hostname(),
            request_id = handler.subscribe_id(),
        ),
        MoqtSessionEvent::Disconnected() => tracing::info_span!(
            parent: session_span,
            "relay.session.disconnected",
            session_id = %session_id,
        ),
        MoqtSessionEvent::Fetch(handler) => tracing::info_span!(
            parent: None,
            "relay.fetch",
            session_id = %session_id,
            app_id,
            peer_relay_id,
            upstream_relay_id = tracing::field::Empty,
            upstream_request_id = tracing::field::Empty,
            relay_hostname = relay_hostname(),
            request_id = handler.request_id(),
            fetch_params = ?handler.fetch_params(),
        ),
        MoqtSessionEvent::ProtocolViolation() => tracing::info_span!(
            parent: session_span,
            "relay.session.protocol_violation",
            session_id = %session_id,
        ),
        MoqtSessionEvent::GoAway(handler) => tracing::info_span!(
            parent: None,
            "relay.go_away",
            session_id = %session_id,
            app_id,
            relay_hostname = relay_hostname(),
            new_session_uri = %handler.new_session_uri(),
        ),
        MoqtSessionEvent::PublishNamespaceCancel(handler) => tracing::info_span!(
            parent: None,
            "relay.publish_namespace_cancel",
            session_id = %session_id,
            app_id,
            relay_hostname = relay_hostname(),
            track_namespace = %handler.track_namespace(),
            error_code = ?handler.error_code(),
            error_reason = %handler.error_reason(),
        ),
        MoqtSessionEvent::PublishDone(handler) => tracing::info_span!(
            parent: None,
            "relay.publish_done",
            session_id = %session_id,
            app_id,
            relay_hostname = relay_hostname(),
            request_id = handler.request_id(),
            status_code = ?handler.status_code(),
            stream_count = handler.stream_count(),
            error_reason = %handler.error_reason(),
        ),
        MoqtSessionEvent::SubscribeUpdate(handler) => tracing::info_span!(
            parent: None,
            "relay.subscribe_update",
            session_id = %session_id,
            app_id,
            relay_hostname = relay_hostname(),
            request_id = handler.request_id(),
            subscription_request_id = handler.subscription_request_id(),
            filter = ?handler.filter(),
            subscriber_priority = handler.subscriber_priority(),
            forward = handler.forward(),
        ),
        MoqtSessionEvent::FetchCancel(handler) => tracing::info_span!(
            parent: None,
            "relay.fetch_cancel",
            session_id = %session_id,
            app_id,
            relay_hostname = relay_hostname(),
            request_id = handler.request_id(),
        ),
        MoqtSessionEvent::TrackStatus(handler) => tracing::info_span!(
            parent: None,
            "relay.track_status",
            session_id = %session_id,
            app_id,
            relay_hostname = relay_hostname(),
            request_id = handler.request_id(),
            track_namespace = %handler.track_namespace(),
            track_name = %handler.track_name(),
        ),
    };
    if let Some(trace_context) = trace_context(event) {
        telemetry::continue_trace_from_traceparent(&span, trace_context.as_str());
    }
    if !matches!(
        event,
        MoqtSessionEvent::Disconnected() | MoqtSessionEvent::ProtocolViolation()
    ) {
        link_span(&span, session_span);
    }
    span
}

fn trace_context(event: &MoqtSessionEvent) -> Option<&TraceContext> {
    match event {
        MoqtSessionEvent::PublishNamespace(handler) => handler.trace_context(),
        MoqtSessionEvent::SubscribeNamespace(handler) => handler.trace_context(),
        MoqtSessionEvent::Publish(handler) => handler.trace_context(),
        MoqtSessionEvent::Subscribe(handler) => handler.trace_context(),
        MoqtSessionEvent::Fetch(handler) => handler.trace_context(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry::global;
    use opentelemetry::trace::{TraceContextExt as _, TracerProvider as _};
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use tracing::Span;
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    use tracing_subscriber::layer::SubscriberExt;

    use super::session_event_span;
    use crate::modules::{
        session::moqt_session_event::MoqtSessionEvent,
        test_support::mock_session::MockSubscribeHandler,
    };

    const TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn a_subscribe_carrying_a_traceparent_continues_the_client_s_trace() {
        // Arrange
        global::set_text_map_propagator(TraceContextPropagator::new());
        let provider = SdkTracerProvider::builder().build();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test")));
        let _default = tracing::subscriber::set_default(subscriber);
        let event = MoqtSessionEvent::Subscribe(Box::new(MockSubscribeHandler {
            trace_context: moqt::TraceContext::parse(TRACEPARENT.as_bytes()),
            ..MockSubscribeHandler::default()
        }));

        // Act
        let span = session_event_span(1, &Span::none(), None, &event);

        // Assert
        let trace_id = span.context().span().span_context().trace_id();
        assert_eq!(trace_id.to_string(), "4bf92f3577b34da6a3ce929d0e0e4736");
    }
}
