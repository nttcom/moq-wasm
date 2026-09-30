import { type LocHeader, readLocHeader } from '../../utils/media/loc'
import { RELAY_CACHE_TTL_MICROS } from './streamConventions'

const ALIGNED_TOLERANCE_MICROS = 500_000

export type GroupRange = { start: bigint; end: bigint }

/// Where the audio groups start on the capture timestamp axis. The live-ingest
/// bridge and the MP4 publisher start an audio group at every video keyframe
/// under the id of the video group; the ONVIF bridge rotates audio groups on
/// its own clock and numbers them independently, so the audio of a review
/// window can only be found from where its groups start.
export class AudioGroups {
  private readonly starts = new Map<bigint, number>()
  private arrivalLagMicros: number | undefined
  newestGroupId: bigint | undefined

  constructor(private readonly nowMicros: () => number) {}

  record(groupId: bigint, locHeader: LocHeader | undefined): void {
    this.noteNewest(groupId)
    const captureMicros = readLocHeader(locHeader).captureTimestampMicros
    if (typeof captureMicros !== 'number' || !Number.isFinite(captureMicros)) {
      return
    }
    this.arrivalLagMicros = this.nowMicros() - captureMicros
    const start = this.starts.get(groupId)
    if (start === undefined || captureMicros < start) {
      this.starts.set(groupId, captureMicros)
    }
    this.forgetBefore(captureMicros - 2 * RELAY_CACHE_TTL_MICROS)
  }

  /// A group known only by its id, as reported by TRACK_STATUS, is placed at
  /// the capture time of the objects live delivery was bringing in last.
  recordLiveGroup(groupId: bigint): void {
    const isNew = this.newestGroupId === undefined || groupId > this.newestGroupId
    this.noteNewest(groupId)
    if (isNew && this.arrivalLagMicros !== undefined && !this.starts.has(groupId)) {
      this.starts.set(groupId, this.nowMicros() - this.arrivalLagMicros)
    }
  }

  reset(): void {
    this.starts.clear()
    this.arrivalLagMicros = undefined
    this.newestGroupId = undefined
  }

  /// Audio groups share the ids of the video groups when every group seen on
  /// both tracks starts at the same time on each; without such a group there is
  /// nothing to tell them apart, and the ids are taken as shared.
  sharesVideoIds(videoStartOf: (groupId: bigint) => number | undefined): boolean {
    for (const [groupId, audioStart] of this.starts) {
      const videoStart = videoStartOf(groupId)
      if (videoStart !== undefined && Math.abs(videoStart - audioStart) > ALIGNED_TOLERANCE_MICROS) {
        return false
      }
    }
    return true
  }

  /// Whether every group that holds audio captured before `untilMicros` has
  /// ended: a later group has started at or after it.
  closedUntil(untilMicros: number): boolean {
    const newest = this.newestGroupId
    const newestStart = newest === undefined ? undefined : this.starts.get(newest)
    return newestStart !== undefined && newestStart >= untilMicros
  }

  /// The closed groups that hold the audio captured from `fromMicros` until
  /// `untilMicros`.
  covering(fromMicros: number, untilMicros: number): GroupRange | undefined {
    const newest = this.newestGroupId
    const closed = [...this.starts]
      .filter(([groupId]) => newest !== undefined && groupId < newest)
      .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
    const first = closed.findLast(([, start]) => start <= fromMicros) ?? closed[0]
    const last = closed.findLast(([, start]) => start < untilMicros)
    if (!first || !last || last[0] < first[0]) {
      return undefined
    }
    return { start: first[0], end: last[0] }
  }

  private forgetBefore(captureMicros: number): void {
    for (const [groupId, start] of this.starts) {
      if (start < captureMicros) {
        this.starts.delete(groupId)
      }
    }
  }

  private noteNewest(groupId: bigint): void {
    if (this.newestGroupId === undefined || groupId > this.newestGroupId) {
      this.newestGroupId = groupId
    }
  }
}
