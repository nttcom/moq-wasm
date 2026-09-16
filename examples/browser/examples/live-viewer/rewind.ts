import { type LocHeader, readLocHeader } from '../../utils/media/loc'

const MICROS_PER_SECOND = 1_000_000

export type GroupMark = {
  groupId: bigint
  captureMicros: number
}

/// Group ids are seeded from a wall clock by the live-ingest bridge and groups
/// last as long as the encoder's GOP, so seconds are resolved from the capture
/// timestamps observed while playing live instead of from group arithmetic.
export class GroupTimeline {
  private readonly marks: GroupMark[] = []
  /// A SUBSCRIBE lands in the middle of the group the publisher is writing, and
  /// the relay starts caching a track only once it has a subscriber, so this
  /// group holds no keyframe for a FETCH replay to start from.
  private joinGroupId: bigint | undefined

  constructor(private readonly capacity: number) {}

  record(groupId: bigint, locHeader?: LocHeader): void {
    const captureMicros = readLocHeader(locHeader).captureTimestampMicros
    if (typeof captureMicros !== 'number' || !Number.isFinite(captureMicros)) {
      return
    }
    this.recordCapture(groupId, captureMicros)
  }

  recordCapture(groupId: bigint, captureMicros: number): void {
    this.joinGroupId ??= groupId
    if (groupId === this.joinGroupId || this.marks.some((mark) => mark.groupId === groupId)) {
      return
    }

    this.marks.push({ groupId, captureMicros })
    this.marks.sort((left, right) => left.captureMicros - right.captureMicros)
    while (this.marks.length > this.capacity) {
      this.marks.shift()
    }
  }

  reset(): void {
    this.marks.length = 0
    this.joinGroupId = undefined
  }

  get latest(): GroupMark | undefined {
    return this.marks[this.marks.length - 1]
  }

  /// The newest group the publisher has finished writing. The live edge group
  /// is still open, and a FETCH that reaches into it leaves the relay cache and
  /// is forwarded to a publisher that does not serve FETCH.
  get newestClosed(): GroupMark | undefined {
    return this.marks[this.marks.length - 2]
  }

  get span(): number {
    const oldest = this.marks[0]
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

    const closed = this.marks.filter((mark) => mark.groupId !== latest.groupId)
    return closed.findLast((mark) => mark.captureMicros <= captureMicros) ?? closed[0]
  }

  secondsBehindLive(groupId: bigint): number {
    const latest = this.latest
    const mark = this.marks.find((candidate) => candidate.groupId === groupId)
    if (!latest || !mark) {
      return 0
    }
    return (latest.captureMicros - mark.captureMicros) / MICROS_PER_SECOND
  }
}

export type ReviewFrame = {
  groupId: bigint
  objectId: bigint
  data: Uint8Array
  captureMicros?: number
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
