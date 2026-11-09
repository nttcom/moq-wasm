/// The `publish()` source of a shared screen, whose tracks are `screenshare`
/// and `screenshare_<rendition>`.
export const SCREEN_SHARE_SOURCE = 'screenshare'

export function isScreenShareTrackName(trackName: string): boolean {
  return trackName.trim().toLowerCase().startsWith(SCREEN_SHARE_SOURCE)
}
