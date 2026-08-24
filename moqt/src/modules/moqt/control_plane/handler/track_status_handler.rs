use std::sync::Arc;

use crate::{
    FilterType, GroupOrder, TransportProtocol,
    modules::moqt::{
        control_plane::{
            control_messages::{
                control_message_type::ControlMessageType, messages::subscribe::Subscribe,
            },
            handler::response_guard::ResponseGuard,
        },
        domains::session_context::SessionContext,
    },
};

/// Responding is not implemented yet: the guard answers TRACK_STATUS_ERROR
/// when the handler is dropped, so the requester does not wait for its
/// control timeout.
#[derive(Debug, Clone)]
pub struct TrackStatusHandler<T: TransportProtocol> {
    _session_context: Arc<SessionContext<T>>,
    request_id: u64,
    track_namespace: String,
    track_name: String,
    subscriber_priority: u8,
    group_order: GroupOrder,
    forward: bool,
    filter_type: FilterType,
    _guard: ResponseGuard<T>,
}

impl<T: TransportProtocol> TrackStatusHandler<T> {
    pub(crate) fn new(session_context: Arc<SessionContext<T>>, track_status: Subscribe) -> Self {
        let guard = ResponseGuard::new(
            session_context.clone(),
            track_status.request_id,
            ControlMessageType::TrackStatusError,
        );
        Self {
            _session_context: session_context,
            request_id: track_status.request_id,
            track_namespace: track_status.track_namespace.join("/"),
            track_name: track_status.track_name,
            subscriber_priority: track_status.subscriber_priority,
            group_order: track_status.group_order,
            forward: track_status.forward,
            filter_type: track_status.filter_type,
            _guard: guard,
        }
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    pub fn track_namespace(&self) -> &str {
        &self.track_namespace
    }

    pub fn track_name(&self) -> &str {
        &self.track_name
    }

    pub fn subscriber_priority(&self) -> u8 {
        self.subscriber_priority
    }

    pub fn group_order(&self) -> GroupOrder {
        self.group_order
    }

    pub fn forward(&self) -> bool {
        self.forward
    }

    pub fn filter_type(&self) -> FilterType {
        self.filter_type
    }
}
