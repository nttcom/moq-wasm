import { AudioGroups } from './audioGroups'
import { MediaTimeline } from './mediaTimeline'
import { GroupTimeline } from './rewind'
import { RELAY_CACHE_TTL_MICROS } from './streamConventions'
import type { MediaCatalogTrack } from '../catalog'

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
  private mediaTrack: { name: string; depends: string[] } | undefined

  get mediaTimelineTrackName(): string | undefined {
    return this.mediaTrack?.name
  }

  /// `true` for the first timeline, which the player then subscribes to. A
  /// later call for that timeline replaces its `depends`, which change as the
  /// source's tracks are added and removed; any other timeline is ignored.
  followMediaTimeline(track: MediaCatalogTrack): boolean {
    if (this.mediaTrack) {
      if (this.mediaTrack.name === track.name) {
        this.mediaTrack.depends = track.depends ?? []
      }
      return false
    }
    this.mediaTrack = { name: track.name, depends: track.depends ?? [] }
    return true
  }

  /// The groups the timeline lists become seek targets only while it depends
  /// on `videoTrackName`, the video track played: another source numbers its
  /// groups on its own.
  replaceMediaTimeline(document: string, videoTrackName: string | undefined): void {
    this.media.replace(document)
    this.groups.replaceListed(this.stampedByMediaTimeline(videoTrackName) ? this.media.groupMarks() : [])
  }

  stampedByMediaTimeline(trackName: string | undefined): boolean {
    return trackName !== undefined && (this.mediaTrack?.depends.includes(trackName) ?? false)
  }

  /// A media timeline that depends on both tracks says they share the ids; it
  /// also lists groups whose audio starts were never observed. Otherwise the
  /// groups observed on both tracks tell.
  audioSharesVideoIds(videoTrackName: string | undefined, audioTrackName: string): boolean {
    if (this.stampedByMediaTimeline(videoTrackName) && this.stampedByMediaTimeline(audioTrackName)) {
      return true
    }
    return this.audio.sharesVideoIds((groupId) => this.groups.startOf(groupId))
  }

  reset(): void {
    this.groups.reset()
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
}
