export type CatalogSubscribeRole = 'video' | 'screenshare' | 'audio' | 'chat'

/// The selection of a video role that leaves the rendition to the player's
/// automatic choice; track names never start with `@`.
export const AUTO_TRACK_SELECTION = '@auto'

/// In frames: 2 s at 30 fps. A viewer switching to a rendition that the relay
/// already receives starts at its next keyframe, since the relay does not pass
/// that SUBSCRIBE on to the publisher.
export const DEFAULT_VIDEO_KEYFRAME_INTERVAL = 60
