import { deserializeChunk } from './chunk'
import type { JitterBufferSubgroupObject, SubgroupObject } from './jitterBufferTypes'

const DEFAULT_MIN_DELAY_MS = 250
const DEFAULT_BUFFERED_AHEAD_FRAMES = 5
const DEFAULT_JITTER_BUFFER_SIZE = 9000
const OBJECT_STATUS_END_OF_GROUP = 3
const MIN_FRAME_INTERVAL_MS = 20

export type VideoJitterBufferMode = 'normal' | 'correctly' | 'fast' | 'buffered'

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
  private lastPopWallTime: number | null = null
  private pendingEndGroupTail: Map<bigint, bigint> = new Map()
  private bufferedAheadFrames = DEFAULT_BUFFERED_AHEAD_FRAMES

  constructor(
    private readonly maxBufferSize: number = DEFAULT_JITTER_BUFFER_SIZE,
    private readonly mode: VideoJitterBufferMode = 'fast',
    keyframeInterval?: number | bigint
  ) {
    this.keyframeInterval =
      typeof keyframeInterval === 'number' ? BigInt(keyframeInterval) : (keyframeInterval ?? undefined)
  }

  setMinDelay(minDelayMs: number): void {
    if (minDelayMs < 0) {
      return
    }
    this.minDelayMs = minDelayMs
  }

  setBufferedAheadFrames(frames: number): void {
    if (!Number.isFinite(frames) || frames <= 0) {
      return
    }
    this.bufferedAheadFrames = Math.max(1, Math.floor(frames))
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
    if (typeof parsed.metadata.sentAt === 'number') {
      onReceiveLatency?.(Date.now() - parsed.metadata.sentAt)
    }

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

    if (this.mode === 'buffered') {
      return this.popBufferedMode()
    }

    if (this.mode === 'fast') {
      return this.popFastMode()
    }

    if (this.mode === 'normal') {
      return this.popNormalMode()
    }

    return this.popCorrectlyMode()
  }

  private popBufferedMode(): VideoJitterBufferEntry | null {
    while (true) {
      if (this.buffer.length === 0) {
        return null
      }

      const headGroup = this.buffer[0].groupId
      const hasTwoAhead = this.buffer.some((entry) => entry.groupId >= headGroup + 2n)
      if (hasTwoAhead) {
        // 現在の GoP 全体を破棄し、次の GoP へ進む
        while (this.buffer.length > 0 && this.buffer[0].groupId === headGroup) {
          this.buffer.shift()
        }
        continue
      }

      if (this.buffer.length <= this.bufferedAheadFrames) {
        return null
      }

      const head = this.buffer.shift()
      if (!head) {
        return null
      }
      this.recordPopResult(head, Date.now())
      return head
    }
  }

  private popFastMode(): VideoJitterBufferEntry | null {
    if (this.buffer.length === 0) {
      return null
    }

    // 次のGoPが始まっていたら、現在のGoPをスキップして次のGoPに進む
    const headGroup = this.buffer[0].groupId
    const hasNextGroup = this.buffer.some((entry) => entry.groupId > headGroup)

    if (hasNextGroup) {
      // 現在のGoP全体を破棄し、次のGoPへ進む
      while (this.buffer.length > 0 && this.buffer[0].groupId === headGroup) {
        this.buffer.shift()
      }
    }

    const head = this.buffer.shift()
    if (!head) {
      return null
    }
    this.recordPopResult(head, Date.now())
    return head
  }

  private popNormalMode(): VideoJitterBufferEntry | null {
    const head = this.buffer[0]
    const nowMs = Date.now()
    const base = head.sentAt
    const delayMs = nowMs - base
    if (delayMs < this.minDelayMs) {
      return null
    }

    this.buffer.shift()
    this.recordPopResult(head, nowMs)
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
    const nowMs = Date.now()
    const base = entry.sentAt
    const delayMs = nowMs - base
    const sinceLastPop = this.lastPopWallTime === null ? Number.POSITIVE_INFINITY : nowMs - this.lastPopWallTime
    if (delayMs < this.minDelayMs || sinceLastPop < MIN_FRAME_INTERVAL_MS) {
      return null
    }

    this.buffer.splice(index, 1)
    this.recordPopResult(entry, nowMs)
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

  private recordPopResult(entry: VideoJitterBufferEntry, nowMs: number): void {
    this.lastPopWallTime = nowMs
    this.lastPoppedGroupId = entry.groupId
    this.lastPoppedObjectId = entry.objectId

    if (entry.isEndOfGroup) {
      this.pendingEndGroupTail.set(entry.groupId, entry.objectId)
    }
  }

  private getExpectedNextEntry(): { groupId: bigint; objectId: bigint } {
    if (this.lastPoppedGroupId === null || this.lastPoppedObjectId === null) {
      return { groupId: 0n, objectId: 0n }
    }

    // EndOfGroup が来ていて、かつそこまでポップ済みなら次グループへ
    const pendingTail = this.pendingEndGroupTail.get(this.lastPoppedGroupId)
    if (pendingTail !== undefined && this.lastPoppedObjectId >= pendingTail) {
      return { groupId: this.lastPoppedGroupId + 1n, objectId: 0n }
    }

    // keyframeInterval による推測は残す（終端が見つからない場合のフォールバック）
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
