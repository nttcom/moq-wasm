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
