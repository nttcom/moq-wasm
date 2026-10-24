use crate::modules::moqt::control_plane::control_messages::key_value_pair::KeyValuePair;

/// What PUBLISH, SUBSCRIBE_OK and FETCH_OK say about the track:
/// parameters in draft-14, Track Properties in draft-18 (§2.5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackProperties {
    /// MAX_CACHE_DURATION in milliseconds (draft-14 §9.2.1.3, draft-18
    /// §12.3).
    pub max_cache_duration: Option<u64>,
    /// The received draft-18 Track Properties, in received order, except
    /// MAX_CACHE_DURATION and (outside FETCH_OK) DEFAULT PUBLISHER GROUP ORDER
    /// in the mutable list, which have fields. Immutable Properties (§12.7)
    /// stay whole. On send these go out unchanged and win over a field of the
    /// same type (§2.5). Draft-14 neither fills nor sends this.
    pub other: Vec<KeyValuePair>,
}
