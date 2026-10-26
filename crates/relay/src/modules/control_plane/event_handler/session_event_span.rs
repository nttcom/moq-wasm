use opentelemetry::trace::TraceContextExt as _;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::modules::{
    domain::session_id::SessionId, session::moqt_session_event::MoqtSessionEvent,
};

/// A request's span roots its own trace, linked to the session's: the
/// session's trace keeps only its lifecycle, `Disconnected` and
/// `ProtocolViolation`.
pub(super) fn session_event_span(
    session_id: SessionId,
    session_span: &Span,
    app_id: Option<&str>,
    event: &MoqtSessionEvent,
) -> Span {
    let span = match event {
        MoqtSessionEvent::PublishNamespace(handler) => tracing::info_span!(
            parent: None,
            "relay.publish_namespace",
            session_id = %session_id,
            app_id,
            track_namespace = %handler.track_namespace(),
        ),
        MoqtSessionEvent::PublishNamespaceDone(handler) => tracing::info_span!(
            parent: None,
            "relay.publish_namespace_done",
            session_id = %session_id,
            app_id,
            track_namespace = %handler.track_namespace(),
        ),
        MoqtSessionEvent::SubscribeNamespace(handler) => tracing::info_span!(
            parent: None,
            "relay.subscribe_namespace",
            session_id = %session_id,
            app_id,
            track_namespace_prefix = %handler.track_namespace_prefix(),
        ),
        MoqtSessionEvent::UnsubscribeNamespace(handler) => tracing::info_span!(
            parent: None,
            "relay.unsubscribe_namespace",
            session_id = %session_id,
            app_id,
            track_namespace_prefix = %handler.track_namespace_prefix(),
        ),
        MoqtSessionEvent::Publish(handler) => tracing::info_span!(
            parent: None,
            "relay.publish",
            session_id = %session_id,
            app_id,
            track_namespace = %handler.track_namespace(),
            track_name = %handler.track_name(),
            track_alias = handler.track_alias(),
            group_order = ?handler._group_order(),
            content_exists = ?handler._content_exists(),
            forward = handler._forward(),
            delivery_timeout = ?handler._delivery_timeout(),
            max_cache_duration = ?handler._max_cache_duration(),
        ),
        MoqtSessionEvent::Subscribe(handler) => tracing::info_span!(
            parent: None,
            "relay.subscribe",
            session_id = %session_id,
            app_id,
            subscribe_id = handler.subscribe_id(),
            track_namespace = %handler.track_namespace(),
            track_name = %handler.track_name(),
            subscriber_priority = handler._subscriber_priority(),
            group_order = ?handler._group_order(),
            forward = handler._forward(),
            filter_type = ?handler._filter_type(),
            max_cache_duration = ?handler._max_cache_duration(),
            delivery_timeout = ?handler._delivery_timeout(),
        ),
        MoqtSessionEvent::Unsubscribe(handler) => tracing::info_span!(
            parent: None,
            "relay.unsubscribe",
            session_id = %session_id,
            app_id,
            subscribe_id = handler.subscribe_id(),
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
            new_session_uri = %handler.new_session_uri(),
        ),
        MoqtSessionEvent::MaxRequestId(handler) => tracing::info_span!(
            parent: None,
            "relay.max_request_id",
            session_id = %session_id,
            app_id,
            request_id = handler.request_id(),
        ),
        MoqtSessionEvent::RequestsBlocked(handler) => tracing::info_span!(
            parent: None,
            "relay.requests_blocked",
            session_id = %session_id,
            app_id,
            maximum_request_id = handler.maximum_request_id(),
        ),
        MoqtSessionEvent::PublishNamespaceCancel(handler) => tracing::info_span!(
            parent: None,
            "relay.publish_namespace_cancel",
            session_id = %session_id,
            app_id,
            track_namespace = %handler.track_namespace(),
            error_code = ?handler.error_code(),
            error_reason = %handler.error_reason(),
        ),
        MoqtSessionEvent::PublishDone(handler) => tracing::info_span!(
            parent: None,
            "relay.publish_done",
            session_id = %session_id,
            app_id,
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
            request_id = handler.request_id(),
            subscription_request_id = handler.subscription_request_id(),
            start_location = ?handler.start_location(),
            end_group = handler.end_group(),
            subscriber_priority = handler.subscriber_priority(),
            forward = handler.forward(),
        ),
        MoqtSessionEvent::FetchCancel(handler) => tracing::info_span!(
            parent: None,
            "relay.fetch_cancel",
            session_id = %session_id,
            app_id,
            request_id = handler.request_id(),
        ),
        MoqtSessionEvent::TrackStatus(handler) => tracing::info_span!(
            parent: None,
            "relay.track_status",
            session_id = %session_id,
            app_id,
            request_id = handler.request_id(),
            track_namespace = %handler.track_namespace(),
            track_name = %handler.track_name(),
        ),
    };
    if !matches!(
        event,
        MoqtSessionEvent::Disconnected() | MoqtSessionEvent::ProtocolViolation()
    ) {
        link_to_session(&span, session_span);
    }
    span
}

pub(super) fn link_to_session(span: &Span, session_span: &Span) {
    span.add_link(session_span.context().span().span_context().clone());
}
