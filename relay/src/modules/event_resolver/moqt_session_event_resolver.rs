use crate::modules::{
    core::session_event::SessionEvent, enums::MOQTMessageReceived, types::SessionId,
};

pub(crate) struct MOQTSessionEventResolver;

impl MOQTSessionEventResolver {
    pub(crate) fn resolve(session_id: SessionId, event: SessionEvent) -> MOQTMessageReceived {
        match event {
            SessionEvent::PublishNamespace(handler) => {
                MOQTMessageReceived::PublishNameSpace(session_id, handler)
            }
            SessionEvent::SubscribeNamespace(handler) => {
                MOQTMessageReceived::SubscribeNameSpace(session_id, handler)
            }
            SessionEvent::Publish(handler) => MOQTMessageReceived::Publish(session_id, handler),
            SessionEvent::Subscribe(handler) => MOQTMessageReceived::Subscribe(session_id, handler),
            SessionEvent::ProtocolViolation() => MOQTMessageReceived::ProtocolViolation(),
        }
    }
}
