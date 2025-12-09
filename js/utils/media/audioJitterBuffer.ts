import { deserializeChunk } from './chunk'
import type { JitterBufferSubgroupObject, SubgroupObject } from './jitterBufferTypes'

const INITIAL_CATCHUP_DELAY_MS = 1000
const DEFAULT_JITTER_BUFFER_SIZE = 1800

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
  private firstInsertTimestamp: number | null = null

  constructor(private readonly maxBufferSize: number = DEFAULT_JITTER_BUFFER_SIZE) {}

  push(
    groupId: bigint,
    objectId: bigint,
    object: SubgroupObject,
    onReceiveLatency?: (latencyMs: number) => void
  ): void {
    if (!object.objectPayloadLength) {
      return
    }
    const parsed = deserializeChunk(object.objectPayload)
    if (!parsed) {
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

    if (this.firstInsertTimestamp === null) {
      this.firstInsertTimestamp = insertTimestamp
    }

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

    if (!this.hasPoppedOnce) {
      if (
        this.firstInsertTimestamp === null ||
        performance.now() - this.firstInsertTimestamp < INITIAL_CATCHUP_DELAY_MS
      ) {
        return null
      }
      const firstConfigIndex = this.buffer.findIndex(
        (entry) =>
          !!entry.object.cachedChunk?.metadata?.codec || !!entry.object.cachedChunk?.metadata?.descriptionBase64
      )
      const index = firstConfigIndex >= 0 ? firstConfigIndex : 0
      const entry = this.buffer.splice(index, 1)[0]
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
