use crate::modules::{
    core::session_event::MoqtSessionEvent, session_event::SessionEvent, types::SessionId,
};

pub(crate) struct RelaySessionEventResolver;

impl RelaySessionEventResolver {
    pub(crate) fn resolve(session_id: SessionId, event: MoqtSessionEvent) -> SessionEvent {
        match event {
            MoqtSessionEvent::PublishNamespace(handler) => {
                SessionEvent::PublishNameSpace(session_id, handler)
            }
            MoqtSessionEvent::SubscribeNamespace(handler) => {
                SessionEvent::SubscribeNameSpace(session_id, handler)
            }
            MoqtSessionEvent::Publish(handler) => SessionEvent::Publish(session_id, handler),
            MoqtSessionEvent::Subscribe(handler) => SessionEvent::Subscribe(session_id, handler),
            MoqtSessionEvent::Unsubscribe(handler) => {
                SessionEvent::Unsubscribe(session_id, handler)
            }
            MoqtSessionEvent::Disconnected() => SessionEvent::Disconnected(session_id),
            MoqtSessionEvent::ProtocolViolation() => SessionEvent::ProtocolViolation(session_id),
        }
    }
}
