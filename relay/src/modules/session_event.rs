use crate::modules::{core::session_event::MoqtSessionEvent, types::SessionId};

pub(crate) struct SessionEvent {
    pub(crate) session_id: SessionId,
    pub(crate) kind: EventKind,
}

pub(crate) enum EventKind {
    FromSession(MoqtSessionEvent),
}
