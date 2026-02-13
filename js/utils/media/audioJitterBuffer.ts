import { tryDeserializeChunk, type ChunkMetadata } from './chunk'
import { readLocHeader } from './loc'
import type { JitterBufferSubgroupObject, SubgroupObjectWithLoc } from './jitterBufferTypes'

const DEFAULT_JITTER_BUFFER_SIZE = 1800

export type AudioJitterBufferMode = 'ordered' | 'latest'

type AudioJitterBufferEntry = {
  groupId: bigint
  objectId: bigint
  bufferInsertTimestamp: number
  sentAt: number
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
  ): void {
    if (!object.objectPayloadLength) {
      return
    }
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
      return
    }

    const bufferObject = object as JitterBufferSubgroupObject
    bufferObject.cachedChunk = parsed
    bufferObject.remotePTS = parsed.metadata.timestamp
    bufferObject.localPTS = performance.timeOrigin + performance.now()
    onReceiveLatency?.(Date.now() - parsed.metadata.sentAt)

    const insertTimestamp = performance.now()
    const entry: AudioJitterBufferEntry = {
      groupId,
      objectId,
      bufferInsertTimestamp: insertTimestamp,
      sentAt: parsed.metadata.sentAt,
      object: bufferObject
    }

    const pos = this.findInsertPos(groupId, objectId)
    this.buffer.splice(pos, 0, entry)

    if (this.buffer.length > this.maxBufferSize) {
      console.warn('[AudioJitterBuffer] Buffer full, dropping oldest entry')
      this.buffer.shift()
    }
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
}

function buildChunkFromLoc(object: SubgroupObjectWithLoc): { metadata: ChunkMetadata; data: Uint8Array } | null {
  const loc = readLocHeader(object.locHeader)
  const captureMicros = loc.captureTimestampMicros
  const sentAt = typeof captureMicros === 'number' ? Math.floor(captureMicros / 1000) : Date.now()
  const metadata: ChunkMetadata = {
    type: 'key',
    timestamp: typeof captureMicros === 'number' ? captureMicros : 0,
    duration: null,
    sentAt
  }
  return { metadata, data: object.objectPayload }
}
