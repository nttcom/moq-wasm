import type { MediaCatalogTrack } from '../../catalog'
import { SeekTimeline } from '../seekTimeline'

export const GROUP_MS = 2_000
export const FIRST_ENCODED_AT_MS = 1_759_924_100_000

/// The encode wallclock of a group of `timelineObject(first, ...)`, in microseconds.
export function groupStartMicros(first: number, groupId: number): number {
  return (FIRST_ENCODED_AT_MS + (groupId - first) * GROUP_MS) * 1_000
}

export function timeline(name: string, depends: string[]): MediaCatalogTrack {
  return { name, label: name, packaging: 'mediatimeline', depends }
}

/// A media timeline object that lists groups `first` to `last`, one every `GROUP_MS`.
export function timelineObject(first: number, last: number): string {
  const records = []
  for (let groupId = first; groupId <= last; groupId += 1) {
    const presentationMs = (groupId - first) * GROUP_MS
    records.push([presentationMs, [groupId, 0], FIRST_ENCODED_AT_MS + presentationMs])
  }
  return JSON.stringify(records)
}

export function followingVideoTimeline(): SeekTimeline {
  const seek = new SeekTimeline()
  seek.followMediaTimeline(timeline('video_timeline', ['video', 'audio']))
  return seek
}

/// The `video` and `audio` of a VOD broadcast.
export function vodTracks(trackDuration: number | undefined): MediaCatalogTrack[] {
  return ['video', 'audio'].map((name) => ({ name, label: name, isLive: false, trackDuration }))
}
