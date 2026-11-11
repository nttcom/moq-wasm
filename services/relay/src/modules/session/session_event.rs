use crate::modules::domain::track_key::TrackKey;

/// A relay-internal event addressed to one session's worker.
pub(crate) enum SessionEvent {
    MalformedTrackDetected(TrackKey),
    /// The track wants another Forward State than the one the session's
    /// upstream subscription to it is in.
    UpstreamForwardChanged(TrackKey),
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
