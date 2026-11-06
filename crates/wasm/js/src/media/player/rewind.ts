import { monotonicUnixMicros } from '../internal/clock'
import { ArrivalLag } from './arrivalLag'
import { type LocHeader, readLocHeader } from '../internal/loc'

const MICROS_PER_SECOND = 1_000_000

export type GroupMark = {
  groupId: bigint
  captureMicros: number
}

type ObservedMark = GroupMark & { observedAtMicros: number }

/// Group ids are seeded from a wall clock by the live-ingest bridge and groups
/// last as long as the encoder's GOP, so seconds are resolved from capture
/// timestamps instead of from group arithmetic. The capture timestamps come
/// from the groups observed while playing live and from the groups a followed
/// media timeline lists. The relay evicts objects by the time it received them, so an
/// observed group is kept for as long as the relay caches it after the viewer
/// first observed it.
export class GroupTimeline {
  private marks: ObservedMark[] = []
  /// The newest media timeline object lists every group the publisher still
  /// keeps (draft-ietf-moq-msf-01 §7.3). The join group and the groups from
  /// before the viewer joined are seek targets too: the relay forwards a FETCH
  /// for what it has not cached to the publisher.
  private listed: GroupMark[] = []
  /// A FETCH found the groups up to this one evicted; the media timeline can
  /// list them for a while longer.
  private forgottenThrough: bigint | undefined
  /// A SUBSCRIBE lands in the middle of the group the publisher is writing, and
  /// the relay starts caching a track only once it has a subscriber, so this
  /// group holds no keyframe for a FETCH replay to start from.
  private joinGroupId: bigint | undefined
  private readonly arrivalLag = new ArrivalLag()

  constructor(private readonly retentionMicros: number) {}

  record(groupId: bigint, locHeader: LocHeader | undefined): void {
    const captureMicros = this.arrivalLag.observe(locHeader)
    if (captureMicros !== undefined) {
      this.recordCapture(groupId, captureMicros)
    }
  }

  recordLiveGroup(groupId: bigint): void {
    const captureMicros = this.arrivalLag.liveCaptureMicros()
    if (captureMicros !== undefined) {
      this.recordCapture(groupId, captureMicros)
    }
  }

  private recordCapture(groupId: bigint, captureMicros: number): void {
    this.joinGroupId ??= groupId
    if (groupId === this.joinGroupId || this.marks.some((mark) => mark.groupId === groupId)) {
      return
    }

    this.marks.push({ groupId, captureMicros, observedAtMicros: monotonicUnixMicros() })
  }

  replaceListed(marks: GroupMark[]): void {
    this.listed = marks
  }

  reset(): void {
    this.marks = []
    this.listed = []
    this.forgottenThrough = undefined
    this.joinGroupId = undefined
    this.arrivalLag.reset()
  }

  get latest(): GroupMark | undefined {
    return this.cached().at(-1)
  }

  /// The newest group the publisher has finished writing. The live edge group
  /// is still open, and a FETCH that reaches into it leaves the relay cache and
  /// is forwarded to a publisher that does not serve FETCH.
  get newestClosed(): GroupMark | undefined {
    return this.cached().at(-2)
  }

  forgetThrough(groupId: bigint): void {
    this.forgottenThrough = groupId
  }

  get span(): number {
    const oldest = this.cached()[0]
    const latest = this.latest
    if (!oldest || !latest) {
      return 0
    }
    return (latest.captureMicros - oldest.captureMicros) / MICROS_PER_SECOND
  }

  /// Resolves the newest closed group that starts at or before `captureMicros`,
  /// or the oldest closed one when the position lies before all of them. The
  /// live edge group is excluded because the publisher is still writing to it
  /// and a FETCH for an open group escapes the relay cache.
  resolveSeekTarget(captureMicros: number): GroupMark | undefined {
    const latest = this.latest
    if (!latest) {
      return undefined
    }

    const closed = this.cached().filter((mark) => mark.groupId !== latest.groupId)
    return closed.findLast((mark) => mark.captureMicros <= captureMicros) ?? closed[0]
  }

  startOf(groupId: bigint): number | undefined {
    return this.cached().find((mark) => mark.groupId === groupId)?.captureMicros
  }

  startAfter(groupId: bigint): number | undefined {
    return this.cached().find((mark) => mark.groupId > groupId)?.captureMicros
  }

  secondsBehindLive(groupId: bigint): number {
    const latest = this.latest
    const mark = this.cached().find((candidate) => candidate.groupId === groupId)
    if (!latest || !mark) {
      return 0
    }
    return (latest.captureMicros - mark.captureMicros) / MICROS_PER_SECOND
  }

  private cached(): GroupMark[] {
    const evictedBeforeMicros = monotonicUnixMicros() - this.retentionMicros
    this.marks = this.marks.filter((mark) => mark.observedAtMicros >= evictedBeforeMicros)
    const observed = new Set(this.marks.map(({ groupId }) => groupId))
    return [...this.marks, ...this.listed.filter(({ groupId }) => !observed.has(groupId))]
      .filter(({ groupId }) => this.forgottenThrough === undefined || groupId > this.forgottenThrough)
      .sort((left, right) => left.captureMicros - right.captureMicros)
  }
}

export type ReviewFrame = {
  groupId: bigint
  objectId: bigint
  data: Uint8Array
  captureMicros?: number
  requestId?: bigint
}

export function toReviewFrame(message: {
  groupId: bigint
  objectId: bigint
  objectPayload: Uint8Array
  locHeader?: LocHeader
}): ReviewFrame | undefined {
  const data = new Uint8Array(message.objectPayload)
  if (data.byteLength === 0) {
    return undefined
  }

  return {
    groupId: message.groupId,
    objectId: message.objectId,
    data,
    captureMicros: readLocHeader(message.locHeader).captureTimestampMicros
  }
}

export function sortReviewFrames(frames: ReviewFrame[]): ReviewFrame[] {
  return [...frames].sort((left, right) =>
    left.groupId === right.groupId ? Number(left.objectId - right.objectId) : Number(left.groupId - right.groupId)
  )
}
