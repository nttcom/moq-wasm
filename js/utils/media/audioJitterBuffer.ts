import { deserializeChunk } from './chunk'
import type { JitterBufferSubgroupObject, SubgroupObject } from './jitterBufferTypes'

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
