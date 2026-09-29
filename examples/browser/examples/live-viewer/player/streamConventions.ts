/// The relay's default RELAY_CACHE_TTL_SECS: draft-ietf-moq-transport-14 gives
/// a subscriber no way to ask which groups the relay still caches.
export const RELAY_CACHE_TTL_MICROS = 60_000_000

/// The bridge lists a CMAF sibling next to every LOC track under this suffix.
export const CMAF_TRACK_SUFFIX = '_cmaf'

/// A video track whose catalog entry carries no `initData` sends its parameter
/// sets in band.
export const ANNEX_B_FORMAT = 'annexb'
