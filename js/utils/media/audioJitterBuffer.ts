import { tryDeserializeChunk, type ChunkMetadata } from './chunk'
import { readLocHeader } from './loc'
import { latencyMsFromCaptureMicros } from './clock'
import type { JitterBufferSubgroupObject, SubgroupObjectWithLoc } from './jitterBufferTypes'

const DEFAULT_JITTER_BUFFER_SIZE = 1800

export type AudioJitterBufferMode = 'ordered' | 'latest'

type AudioJitterBufferEntry = {
  groupId: bigint
  objectId: bigint
  bufferInsertTimestamp: number
  captureTimestampMicros?: number
  object: JitterBufferSubgroupObject
}

export class AudioJitterBuffer {
  private buffer: AudioJitterBufferEntry[] = []
  private hasPoppedOnce = false
  private mode: AudioJitterBufferMode = 'ordered'

  constructor(
    private readonly maxBufferSize: number = DEFAULT_JITTER_BUFFER_SIZE,
    mode: AudioJitterBufferMode = 'ordered'
  ) {
    this.mode = mode
  }

  setMode(mode: AudioJitterBufferMode): void {
    this.mode = mode
  }

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

    const parsedFromMetadata = tryDeserializeChunk(object.objectPayload)
    if (!parsedFromMetadata) {
      const locHeader = object.locHeader
      const extensions = locHeader?.extensions ?? []
      if (!locHeader || extensions.length === 0) {
        console.debug('[AudioJitterBuffer] Missing metadata and LOC header', {
          groupId,
          objectId,
          payloadLength: object.objectPayloadLength
        })
      } else {
        const hasCaptureTimestamp = extensions.some((ext) => ext.type === 'captureTimestamp')
        console.debug('[AudioJitterBuffer] Using LOC header fallback', {
          groupId,
          objectId,
          payloadLength: object.objectPayloadLength,
          locExtensionCount: extensions.length,
          hasCaptureTimestamp
        })
      }
    }
    const parsed = parsedFromMetadata ?? buildChunkFromLoc(object)
    if (!parsed) {
      console.debug('[AudioJitterBuffer] Failed to build chunk from payload', {
        groupId,
        objectId,
        payloadLength: object.objectPayloadLength
      })
      return false
    }

    const bufferObject = object as JitterBufferSubgroupObject
    bufferObject.cachedChunk = parsed
    bufferObject.remotePTS = parsed.metadata.timestamp
    bufferObject.localPTS = performance.timeOrigin + performance.now()
    if (typeof captureTimestampMicros === 'number') {
      onReceiveLatency?.(latencyMsFromCaptureMicros(captureTimestampMicros))
    }

    const insertTimestamp = performance.now()
    const entry: AudioJitterBufferEntry = {
      groupId,
      objectId,
      bufferInsertTimestamp: insertTimestamp,
      captureTimestampMicros,
      object: bufferObject
    }

    const pos = this.findInsertPos(groupId, objectId)
    this.buffer.splice(pos, 0, entry)

    if (this.buffer.length > this.maxBufferSize) {
      console.warn('[AudioJitterBuffer] Buffer full, dropping oldest entry')
      this.buffer.shift()
    }
    return true
  }

  pop(): AudioJitterBufferEntry | null {
    return this.popWithMetadata()
  }

  popWithMetadata(): AudioJitterBufferEntry | null {
    if (this.buffer.length === 0) {
      return null
    }

    if (this.mode === 'latest') {
      // latestモード: 常に最新のデータを取り出し、それ以前のデータを消去
      const entry = this.buffer.pop()
      this.buffer.length = 0 // それ以前のデータを全て消去
      return entry ?? null
    }

    // orderedモード: 古いデータから順に取り出す
    if (!this.hasPoppedOnce) {
      // 初回は一番最後に挿入したデータ（最新のデータ）を取り出す
      const entry = this.buffer.pop()
      this.hasPoppedOnce = true
      return entry ?? null
    }

    const head = this.buffer[0]
    this.buffer.shift()
    return head
  }

  private findInsertPos(groupId: bigint, objectId: bigint): number {
    for (let i = this.buffer.length - 1; i >= 0; i--) {
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

function buildChunkFromLoc(object: SubgroupObjectWithLoc): { metadata: ChunkMetadata; data: Uint8Array } | null {
  const loc = readLocHeader(object.locHeader)
  const captureMicros = getCaptureTimestampMicros(loc.captureTimestampMicros)
  const metadata: ChunkMetadata = {
    type: 'key',
    timestamp: typeof captureMicros === 'number' ? captureMicros : 0,
    duration: null
  }
  return { metadata, data: object.objectPayload }
}

function getCaptureTimestampMicros(value: number | undefined): number | undefined {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) {
    return undefined
  }
  return value
}
