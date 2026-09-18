import { readLocHeader } from './loc'
import { buildVideoChunkFromLoc, getCaptureTimestampMicros } from './locChunk'
import { normalizeSubgroupId, trackSubgroupObjectId } from './subgroupObjectId'
import { latencyMsFromCaptureMicros } from './clock'
import { OBJECT_STATUS_END_OF_GROUP } from './objectStatus'
import type { JitterBufferSubgroupObject, SubgroupObjectWithLoc } from './jitterBufferTypes'

const DEFAULT_JITTER_BUFFER_SIZE = 9000

type VideoJitterBufferEntry = {
  groupId: bigint
  objectId: bigint
  subgroupId: bigint
  captureTimestampMicros?: number
  object: JitterBufferSubgroupObject
  isEndOfGroup: boolean
}

export class VideoJitterBuffer {
  private buffer: VideoJitterBufferEntry[] = []
  private readonly lastObjectIds = new Map<string, bigint>()

  constructor(private readonly maxBufferSize: number = DEFAULT_JITTER_BUFFER_SIZE) {}

  push(groupId: bigint, object: SubgroupObjectWithLoc, onReceiveLatency?: (latencyMs: number) => void): bigint | null {
    const subgroupId = normalizeSubgroupId(object.subgroupId)
    const objectId = trackSubgroupObjectId(
      this.lastObjectIds,
      groupId,
      object.subgroupId,
      object.objectIdDelta,
      object.objectStatus
    )
    if (!object.objectPayloadLength) {
      return null
    }

    const locMetadata = readLocHeader(object.locHeader)
    const captureTimestampMicros = getCaptureTimestampMicros(locMetadata.captureTimestampMicros)
    const parsed = buildVideoChunkFromLoc(object, objectId)

    const bufferObject: JitterBufferSubgroupObject = {
      ...object,
      objectId,
      cachedChunk: parsed,
      remotePTS: parsed.metadata.timestamp,
      localPTS: performance.timeOrigin + performance.now()
    }

    if (typeof captureTimestampMicros === 'number') {
      onReceiveLatency?.(latencyMsFromCaptureMicros(captureTimestampMicros))
    }

    const entry: VideoJitterBufferEntry = {
      groupId,
      objectId,
      subgroupId,
      captureTimestampMicros,
      object: bufferObject,
      isEndOfGroup: object.objectStatus === OBJECT_STATUS_END_OF_GROUP
    }
    const pos = this.findInsertPos(groupId, objectId, subgroupId)
    this.buffer.splice(pos, 0, entry)

    if (this.buffer.length > this.maxBufferSize) {
      this.buffer.shift()
    }
    return objectId
  }

  pop(): VideoJitterBufferEntry | null {
    return this.popHolding()
  }

  popWithMetadata(): VideoJitterBufferEntry | null {
    return this.popHolding()
  }

  popHolding(): VideoJitterBufferEntry | null {
    return this.buffer.shift() ?? null
  }

  private findInsertPos(groupId: bigint, objectId: bigint, subgroupId: bigint): number {
    for (let i = this.buffer.length - 1; i >= 0; i -= 1) {
      const entry = this.buffer[i]
      if (entry.groupId === groupId && entry.objectId < objectId) {
        return i + 1
      }
      if (entry.groupId === groupId && entry.objectId === objectId && entry.subgroupId <= subgroupId) {
        return i + 1
      }
      if (entry.groupId < groupId) {
        return i + 1
      }
    }
    return 0
  }

  getBufferedFrameCount(): number {
    return this.buffer.length
  }

  getMaxBufferSize(): number {
    return this.maxBufferSize
  }
}
