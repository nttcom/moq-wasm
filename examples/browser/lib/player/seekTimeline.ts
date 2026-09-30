import type { MediaCatalogTrack } from '../../examples/media/catalog'
import { AudioGroups } from './audioGroups'
import { MediaTimeline } from './mediaTimeline'
import { GroupTimeline } from './rewind'
import { RELAY_CACHE_TTL_MICROS } from './streamConventions'

const MICROS_PER_SECOND = 1_000_000

export type SeekAxis = {
  liveEdgeSeconds: number
  replayableStartSeconds: number
  broadcastStartSeconds: number | undefined
  replayableSeconds: number
  seekable: boolean
}

export class SeekTimeline {
  readonly groups = new GroupTimeline(RELAY_CACHE_TTL_MICROS)
  readonly media = new MediaTimeline()
  readonly audio = new AudioGroups()
  private readonly unstampedGroups = new Set<bigint>()
  private mediaTrack: { name: string; depends: string[] } | undefined

  get mediaTimelineTrackName(): string | undefined {
    return this.mediaTrack?.name
  }

  followMediaTimeline(track: MediaCatalogTrack): boolean {
    if (this.mediaTrack) {
      return false
    }
    this.mediaTrack = { name: track.name, depends: track.depends ?? [] }
    return true
  }

  replaceMediaTimeline(document: string): void {
    this.media.replace(document)
    this.stampObservedGroups()
  }

  stampedByMediaTimeline(trackName: string | undefined): boolean {
    return trackName !== undefined && (this.mediaTrack?.depends.includes(trackName) ?? false)
  }

  /// A group observed on a CMAF track, whose objects carry no LOC header, or
  /// reported by TRACK_STATUS is stamped with the encode wallclock the media
  /// timeline records for it. Only observed groups enter the timeline: the relay
  /// caches a track from its first subscriber on, so earlier groups the media
  /// timeline lists cannot be fetched.
  observeTimelineGroup(groupId: bigint): void {
    this.unstampedGroups.add(groupId)
    this.stampObservedGroups()
  }

  resetGroups(): void {
    this.groups.reset()
    this.unstampedGroups.clear()
  }

  reset(): void {
    this.resetGroups()
    this.media.reset()
    this.mediaTrack = undefined
    this.audio.reset()
  }

  /// The axis runs from the start of the broadcast, which the media timeline
  /// places, so the bar keeps its meaning as cache retention grows. Until the
  /// first timeline object arrives it falls back to the replayable window.
  axis(): SeekAxis {
    const liveEdgeSeconds = (this.groups.latest?.captureMicros ?? 0) / MICROS_PER_SECOND
    const replayableSeconds = this.groups.span
    const broadcastStart = this.media.broadcastStartMicros()
    return {
      liveEdgeSeconds,
      replayableStartSeconds: liveEdgeSeconds - replayableSeconds,
      broadcastStartSeconds: broadcastStart === undefined ? undefined : broadcastStart / MICROS_PER_SECOND,
      replayableSeconds,
      seekable: this.groups.newestClosed !== undefined && replayableSeconds > 0
    }
  }

  private stampObservedGroups(): void {
    for (const groupId of this.unstampedGroups) {
      const encodedAtMs = this.media.encodedAtMsFor(groupId)
      if (encodedAtMs !== undefined) {
        this.groups.recordCapture(groupId, encodedAtMs * 1_000)
        this.unstampedGroups.delete(groupId)
      }
    }
  }
}
