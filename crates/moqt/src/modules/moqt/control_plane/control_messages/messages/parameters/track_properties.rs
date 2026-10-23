/// What PUBLISH, SUBSCRIBE_OK and FETCH_OK say about the track itself:
/// parameters in draft-14, Track Properties in draft-18 (§2.5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackProperties {
    /// MAX_CACHE_DURATION in milliseconds (draft-14 §9.2.1.3, draft-18
    /// §12.3).
    pub max_cache_duration: Option<u64>,
}
