import { monotonicUnixMicros } from '../../utils/media/clock'
import { ArrivalLag } from './arrivalLag'
import { type LocHeader, readLocHeader } from '../../utils/media/loc'

const MICROS_PER_SECOND = 1_000_000

export type GroupMark = {
  groupId: bigint
  captureMicros: number
}

type ObservedMark = GroupMark & { observedAtMicros: number }

/// Group ids are seeded from a wall clock by the live-ingest bridge and groups
/// last as long as the encoder's GOP, so seconds are resolved from the capture
/// timestamps observed while playing live instead of from group arithmetic.
/// The relay evicts objects by the time it received them, so a group is kept
/// for as long as the relay caches it after the viewer first observed it.
export class GroupTimeline {
  private marks: ObservedMark[] = []
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

  recordCapture(groupId: bigint, captureMicros: number): void {
    this.joinGroupId ??= groupId
    if (groupId === this.joinGroupId || this.marks.some((mark) => mark.groupId === groupId)) {
      return
    }

    this.marks.push({ groupId, captureMicros, observedAtMicros: monotonicUnixMicros() })
    this.marks.sort((left, right) => left.captureMicros - right.captureMicros)
  }

  reset(): void {
    this.marks = []
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
    this.marks = this.marks.filter((mark) => mark.groupId > groupId)
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

  private cached(): ObservedMark[] {
    const evictedBeforeMicros = monotonicUnixMicros() - this.retentionMicros
    this.marks = this.marks.filter((mark) => mark.observedAtMicros >= evictedBeforeMicros)
    return this.marks
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
