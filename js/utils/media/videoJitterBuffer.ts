import { deserializeChunk } from './chunk'
import type { JitterBufferSubgroupObject, SubgroupObject } from './jitterBufferTypes'

const DEFAULT_MIN_DELAY_MS = 50
const DEFAULT_JITTER_BUFFER_SIZE = 1800
const OBJECT_STATUS_END_OF_GROUP = 3

export type VideoJitterBufferMode = 'normal' | 'correctly'

type VideoJitterBufferEntry = {
  groupId: bigint
  objectId: bigint
  bufferInsertTimestamp: number
  sentAt: number
  object: JitterBufferSubgroupObject
  isEndOfGroup: boolean
}

export class VideoJitterBuffer {
  private buffer: VideoJitterBufferEntry[] = []
  private minDelayMs: number = DEFAULT_MIN_DELAY_MS
  private lastPoppedGroupId: bigint | null = null
  private lastPoppedObjectId: bigint | null = null
  private readonly keyframeInterval?: bigint

  constructor(
    private readonly maxBufferSize: number = DEFAULT_JITTER_BUFFER_SIZE,
    private readonly mode: VideoJitterBufferMode = 'normal',
    keyframeInterval?: number | bigint
  ) {
    this.keyframeInterval =
      typeof keyframeInterval === 'number' ? BigInt(keyframeInterval) : (keyframeInterval ?? undefined)
  }

  setMinDelay(minDelayMs: number): void {
    if (!Number.isFinite(minDelayMs) || minDelayMs < 0) {
      return
    }
    this.minDelayMs = minDelayMs
  }

  push(groupId: bigint, objectId: bigint, object: SubgroupObject): void {
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

    if (this.mode === 'correctly' && this.shouldRejectOldData(groupId, objectId)) {
      console.warn(
        `[VideoJitterBuffer] Rejecting old data. Expected: (group:${this.lastPoppedGroupId}, object:${this.lastPoppedObjectId}), Got: (group:${groupId}, object:${objectId})`
      )
      return
    }

    const entry: VideoJitterBufferEntry = {
      groupId,
      objectId,
      bufferInsertTimestamp: performance.now(),
      sentAt: parsed.metadata.sentAt,
      object: bufferObject,
      isEndOfGroup: object.objectStatus === OBJECT_STATUS_END_OF_GROUP
    }

    const pos = this.findInsertPos(groupId, objectId)
    this.buffer.splice(pos, 0, entry)

    if (this.buffer.length > this.maxBufferSize) {
      console.warn('[VideoJitterBuffer] Buffer full, dropping oldest entry')
      this.buffer.shift()
    }
  }

  pop(): VideoJitterBufferEntry | null {
    return this.popWithMetadata()
  }

  popWithMetadata(): VideoJitterBufferEntry | null {
    if (this.buffer.length === 0) {
      return null
    }

    if (this.mode === 'normal') {
      return this.popNormalMode()
    }

    return this.popCorrectlyMode()
  }

  private popNormalMode(): VideoJitterBufferEntry | null {
    const head = this.buffer[0]
    const delayMs = performance.now() - head.bufferInsertTimestamp
    if (delayMs < this.minDelayMs) {
      return null
    }

    this.buffer.shift()
    this.recordPopResult(head)
    return head
  }

  private popCorrectlyMode(): VideoJitterBufferEntry | null {
    const expectedEntry = this.getExpectedNextEntry()
    let index = this.buffer.findIndex(
      (entry) => entry.groupId === expectedEntry.groupId && entry.objectId === expectedEntry.objectId
    )

    if (index === -1) {
      index = this.findResyncIndex()
      if (index === -1) {
        if (this.lastPoppedGroupId === null && this.buffer.length > 0) {
          const bufferedEntries = this.buffer.slice(0, 5).map((e) => `(g:${e.groupId}, o:${e.objectId})`)
          console.warn(
            `[VideoJitterBuffer] Expected first entry not found. Expected: (g:${expectedEntry.groupId}, o:${expectedEntry.objectId}), Buffer: [${bufferedEntries.join(', ')}]`
          )
        }
        return null
      }
    }

    const entry = this.buffer[index]
    const delayMs = performance.now() - entry.bufferInsertTimestamp
    if (delayMs < this.minDelayMs) {
      return null
    }

    this.buffer.splice(index, 1)
    this.recordPopResult(entry)
    return entry
  }

  private shouldRejectOldData(groupId: bigint, objectId: bigint): boolean {
    if (this.lastPoppedGroupId === null || this.lastPoppedObjectId === null) {
      return false
    }
    if (groupId < this.lastPoppedGroupId) {
      return true
    }
    if (groupId === this.lastPoppedGroupId && objectId <= this.lastPoppedObjectId) {
      return true
    }
    return false
  }

  private recordPopResult(entry: VideoJitterBufferEntry): void {
    this.lastPoppedGroupId = entry.groupId
    this.lastPoppedObjectId = entry.objectId

    if (entry.isEndOfGroup) {
      this.lastPoppedGroupId = entry.groupId + 1n
      this.lastPoppedObjectId = -1n
    }
  }

  private getExpectedNextEntry(): { groupId: bigint; objectId: bigint } {
    if (this.lastPoppedGroupId === null || this.lastPoppedObjectId === null) {
      return { groupId: 0n, objectId: 0n }
    }

    if (this.keyframeInterval !== undefined && this.lastPoppedObjectId === this.keyframeInterval - 1n) {
      return { groupId: this.lastPoppedGroupId + 1n, objectId: 0n }
    }

    return { groupId: this.lastPoppedGroupId, objectId: this.lastPoppedObjectId + 1n }
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

  private findResyncIndex(): number {
    if (this.buffer.length === 0) {
      return -1
    }
    if (this.lastPoppedGroupId === null) {
      return 0
    }
    return this.buffer.findIndex((entry) => entry.objectId === 0n && entry.groupId > this.lastPoppedGroupId!)
  }
}
