use crate::modules::{
    domain::{session_id::SessionId, track_key::TrackKey},
    session::moqt_session_event::MoqtSessionEvent,
};

pub(crate) struct SessionEvent {
    pub(crate) session_id: SessionId,
    pub(crate) kind: EventKind,
}

pub(crate) enum EventKind {
    SessionRegistered,
    FromSession(MoqtSessionEvent),
    MalformedTrackDetected(TrackKey),
    ProtocolViolationDetected { reason: String },
}

impl SessionEvent {
    pub(crate) fn session_registered(session_id: SessionId) -> Self {
        Self {
            session_id,
            kind: EventKind::SessionRegistered,
        }
    }

    pub(crate) fn malformed_track_detected(session_id: SessionId, track_key: TrackKey) -> Self {
        Self {
            session_id,
            kind: EventKind::MalformedTrackDetected(track_key),
        }
    }

    pub(crate) fn protocol_violation_detected(session_id: SessionId, reason: String) -> Self {
        Self {
            session_id,
            kind: EventKind::ProtocolViolationDetected { reason },
        }
    }
}
