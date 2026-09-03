use crate::modules::{
    core::session_event::MoqtSessionEvent,
    types::{SessionId, TrackKey},
};

pub(crate) struct SessionEvent {
    pub(crate) session_id: SessionId,
    pub(crate) kind: EventKind,
}

pub(crate) enum EventKind {
    FromSession(MoqtSessionEvent),
    MalformedTrackDetected(TrackKey),
}

impl SessionEvent {
    pub(crate) fn malformed_track_detected(session_id: SessionId, track_key: TrackKey) -> Self {
        Self {
            session_id,
            kind: EventKind::MalformedTrackDetected(track_key),
        }
    }
}
