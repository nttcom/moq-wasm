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
    ProtocolViolationDetected {
        reason: String,
    },
    GoAwaySent,
    /// The egress runner of the session's downstream subscription `request_id`
    /// delivered its whole range and sent PUBLISH_DONE.
    DownstreamSubscriptionEnded {
        request_id: u64,
    },
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

    pub(crate) fn go_away_sent(session_id: SessionId) -> Self {
        Self {
            session_id,
            kind: EventKind::GoAwaySent,
        }
    }

    pub(crate) fn protocol_violation_detected(session_id: SessionId, reason: String) -> Self {
        Self {
            session_id,
            kind: EventKind::ProtocolViolationDetected { reason },
        }
    }

    pub(crate) fn downstream_subscription_ended(session_id: SessionId, request_id: u64) -> Self {
        Self {
            session_id,
            kind: EventKind::DownstreamSubscriptionEnded { request_id },
        }
    }
}
