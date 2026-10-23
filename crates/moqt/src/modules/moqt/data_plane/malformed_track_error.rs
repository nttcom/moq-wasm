use thiserror::Error;

use crate::modules::moqt::control_plane::control_messages::messages::parameters::location::Location;

/// Why a track is malformed (draft-14 §2.5, draft-18 §2.4.2). The subscriber
/// MUST cancel its subscription and fetches of the track; the receiver only
/// reports it, the caller sends the UNSUBSCRIBE or FETCH_CANCEL.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MalformedTrackError {
    /// A draft-14 track delivered objects on subgroup streams and in
    /// datagrams (draft-14 §2.5 item 9).
    #[error("malformed track: objects arrived on subgroup streams and in datagrams")]
    MixedForwardingPreference,
    /// The object arrived again with another forwarding preference,
    /// subgroup, priority, status or payload (draft-14 §8.1, draft-18 §9.1).
    #[error(
        "malformed track: object {{{}, {}}} arrived again with other properties",
        location.group_id,
        location.object_id
    )]
    ConflictingDuplicate { location: Location },
}
