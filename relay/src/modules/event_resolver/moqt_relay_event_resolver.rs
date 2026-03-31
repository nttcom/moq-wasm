use crate::modules::{core::session_event::SessionEvent, enums::MoqtRelayEvent, types::SessionId};

pub(crate) struct MoqtRelayEventResolver;

impl MoqtRelayEventResolver {
    pub(crate) fn resolve(session_id: SessionId, event: SessionEvent) -> MoqtRelayEvent {
        match event {
            SessionEvent::PublishNamespace(handler) => {
                MoqtRelayEvent::PublishNameSpace(session_id, handler)
            }
            SessionEvent::SubscribeNamespace(handler) => {
                MoqtRelayEvent::SubscribeNameSpace(session_id, handler)
            }
            SessionEvent::Publish(handler) => MoqtRelayEvent::Publish(session_id, handler),
            SessionEvent::Subscribe(handler) => MoqtRelayEvent::Subscribe(session_id, handler),
            SessionEvent::Disconnected() => MoqtRelayEvent::Disconnected(session_id),
            SessionEvent::ProtocolViolation() => MoqtRelayEvent::ProtocolViolation(session_id),
        }
    }
}
