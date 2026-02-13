export function isScreenShareTrackName(trackName: string): boolean {
  return trackName.trim().toLowerCase().startsWith('screenshare')
}
