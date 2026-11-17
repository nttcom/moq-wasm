/// The Subscriber Priority (draft-14 §7.2) a player gives each kind of track;
/// the relay sends a lower value first. The catalog, the text tracks and the
/// audio are small, so they go before a video at the default. A track without
/// a DELIVERY TIMEOUT has to: left behind video a congested link cannot carry,
/// its streams stay open and use up the stream credit the video needs.
export const CATALOG_PRIORITY = 0
export const TEXT_PRIORITY = 16
export const AUDIO_PRIORITY = 32
export const DEFAULT_VIDEO_PRIORITY = 128
