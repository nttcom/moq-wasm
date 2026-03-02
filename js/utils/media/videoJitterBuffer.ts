import { tryDeserializeChunk, type ChunkMetadata } from './chunk'
import { bytesToBase64, readLocHeader } from './loc'
import { latencyMsFromCaptureMicros } from './clock'
import type { JitterBufferSubgroupObject, SubgroupObjectWithLoc } from './jitterBufferTypes'

const DEFAULT_JITTER_BUFFER_SIZE = 9000
const OBJECT_STATUS_END_OF_GROUP = 3

type VideoJitterBufferEntry = {
  groupId: bigint
  objectId: bigint
  captureTimestampMicros?: number
  object: JitterBufferSubgroupObject
  isEndOfGroup: boolean
}

export class VideoJitterBuffer {
  private buffer: VideoJitterBufferEntry[] = []

  constructor(private readonly maxBufferSize: number = DEFAULT_JITTER_BUFFER_SIZE) {}

  push(
    groupId: bigint,
    objectId: bigint,
    object: SubgroupObjectWithLoc,
    onReceiveLatency?: (latencyMs: number) => void
  ): boolean {
    if (!object.objectPayloadLength) {
      return false
    }

    const locMetadata = readLocHeader(object.locHeader)
    const captureTimestampMicros = getCaptureTimestampMicros(locMetadata.captureTimestampMicros)
    const parsed = tryDeserializeChunk(object.objectPayload) ?? buildChunkFromLoc(object, objectId)
    if (!parsed) {
      return false
    }
    if (!parsed.metadata.descriptionBase64 && locMetadata.videoConfig) {
      parsed.metadata.descriptionBase64 = bytesToBase64(locMetadata.videoConfig)
    }

    const bufferObject = object as JitterBufferSubgroupObject
    bufferObject.cachedChunk = parsed
    bufferObject.remotePTS = parsed.metadata.timestamp
    bufferObject.localPTS = performance.timeOrigin + performance.now()

    if (typeof captureTimestampMicros === 'number') {
      onReceiveLatency?.(latencyMsFromCaptureMicros(captureTimestampMicros))
    }

    const entry: VideoJitterBufferEntry = {
      groupId,
      objectId,
      captureTimestampMicros,
      object: bufferObject,
      isEndOfGroup: object.objectStatus === OBJECT_STATUS_END_OF_GROUP
    }
    const pos = this.findInsertPos(groupId, objectId)
    this.buffer.splice(pos, 0, entry)

    if (this.buffer.length > this.maxBufferSize) {
      this.buffer.shift()
    }
    return true
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

  private findInsertPos(groupId: bigint, objectId: bigint): number {
    for (let i = this.buffer.length - 1; i >= 0; i -= 1) {
      const entry = this.buffer[i]
      if (entry.groupId === groupId && entry.objectId < objectId) {
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

function buildChunkFromLoc(
  object: SubgroupObjectWithLoc,
  objectId: bigint
): { metadata: ChunkMetadata; data: Uint8Array } | null {
  const loc = readLocHeader(object.locHeader)
  const captureMicros = getCaptureTimestampMicros(loc.captureTimestampMicros)
  const metadata: ChunkMetadata = {
    type: objectId === 0n ? 'key' : 'delta',
    timestamp: typeof captureMicros === 'number' ? captureMicros : 0,
    duration: null,
    descriptionBase64: loc.videoConfig ? bytesToBase64(loc.videoConfig) : undefined
  }
  return { metadata, data: object.objectPayload }
}

function getCaptureTimestampMicros(value: number | undefined): number | undefined {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) {
    return undefined
  }
  return value
}
