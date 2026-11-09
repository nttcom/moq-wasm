use moqt::object::{ForwardingPreference, Object};

/// The first Subgroup ID of the range egress gives the datagrams of a track
/// it delivers on subgroup streams: the datagram of Object ID `n` goes alone
/// on subgroup `CONVERTED_DATAGRAM_SUBGROUPS + n`. Publishers number their
/// subgroups from 0, so a real subgroup is not expected this high; one that
/// is may share a Subgroup ID with a converted datagram.
pub(crate) const CONVERTED_DATAGRAM_SUBGROUPS: u64 = 1 << 61;

/// The Subgroup ID of the converted datagram of Object ID `object_id`;
/// `None` past the range, below the varint limit of 2^62.
pub(crate) fn converted_datagram_subgroup(object_id: u64) -> Option<u64> {
    (object_id < CONVERTED_DATAGRAM_SUBGROUPS).then_some(CONVERTED_DATAGRAM_SUBGROUPS + object_id)
}

/// The one forwarding preference a track goes in to a session that keeps
/// each track to one (draft-14 §10), whatever the track mixes (draft-18
/// §11). Live delivery and a FETCH from the cache give the session's objects
/// the same form, as its duplicate check compares the preference and the
/// subgroup (draft-14 §8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SinglePreference {
    Subgroups,
    Datagrams,
}

impl SinglePreference {
    /// The preference `object` goes in; `None` for a datagram with no
    /// converted subgroup.
    pub(crate) fn forwarding(self, object: &Object) -> Option<ForwardingPreference> {
        match (self, object.forwarding) {
            (Self::Datagrams, _) => Some(ForwardingPreference::Datagram),
            (Self::Subgroups, ForwardingPreference::Datagram) => {
                converted_datagram_subgroup(object.location.object_id)
                    .map(|subgroup_id| ForwardingPreference::Subgroup { subgroup_id })
            }
            (Self::Subgroups, forwarding) => Some(forwarding),
        }
    }
}
