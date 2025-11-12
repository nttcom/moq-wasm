import { deserializeChunk, type ChunkMetadata, type DeserializedChunk } from './chunk'
import type { SubgroupStreamObjectMessage } from '../../pkg/moqt_client_sample'

const DEFAULT_MIN_DELAY_MS = 50
const DEFAULT_JITTER_BUFFER_SIZE = 1800
const AUDIO_STALE_THRESHOLD_MS = 500
const OBJECT_STATUS_END_OF_GROUP = 3

export type SubgroupObject = Pick<
  SubgroupStreamObjectMessage,
  'objectId' | 'objectPayloadLength' | 'objectPayload' | 'objectStatus'
>

export type SubgroupWorkerMessage = {
  groupId: bigint
  subgroupStreamObject: SubgroupObject
}

export type JitterBufferSubgroupObject = SubgroupObject & { cachedChunk: DeserializedChunk }

export type JitterBufferEntry = {
  groupId: bigint
  objectId: bigint
  timestamp: number
  object: JitterBufferSubgroupObject
  isEndOfGroup: boolean
  referenceTime?: number
}

export type JitterBufferMode = 'video_normal' | 'video_correctly' | 'audio'

/**
 * JitterBuffer is a class that implements the jitter buffer for SubgroupObjects.
 * It stores objects in a sorted order based on groupId and objectId.
 * When reordering, groupId takes precedence over objectID.
 *
 * Modes:
 * - normal: Returns the oldest buffered object based on delay timing
 * - correctly: Ensures strict sequential order, only returns the next expected objectId
 */
export class JitterBuffer {
  private buffer: JitterBufferEntry[] = []
  private minDelayMs: number = DEFAULT_MIN_DELAY_MS
  private lastPoppedGroupId: bigint | null = null
  private lastPoppedObjectId: bigint | null = null
  private readonly keyframeInterval?: bigint

  constructor(
    private readonly maxBufferSize: number = DEFAULT_JITTER_BUFFER_SIZE,
    private readonly mode: JitterBufferMode,
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

  push(groupId: bigint, objectId: bigint, object: SubgroupObject) {
    if (!object.objectPayloadLength) {
      return
    }
    const parsed = deserializeChunk(object.objectPayload)
    if (!parsed) {
      return
    }
    const bufferObject = object as JitterBufferSubgroupObject
    bufferObject.cachedChunk = parsed
    // correctly モードでは古いデータを拒否
    if (this.mode === 'video_correctly' && this.shouldRejectOldData(groupId, objectId)) {
      console.warn(
        `[JitterBuffer] Rejecting old data. Expected: (group:${this.lastPoppedGroupId}, object:${this.lastPoppedObjectId}), Got: (group:${groupId}, object:${objectId})`
      )
      return
    }

    const timestamp = performance.now()
    const entry: JitterBufferEntry = {
      groupId,
      objectId,
      timestamp,
      object: bufferObject,
      isEndOfGroup: object.objectStatus === OBJECT_STATUS_END_OF_GROUP,
      referenceTime: computeReferenceTime(parsed.metadata)
    }

    const pos = this.findInsertPos(groupId, objectId)
    this.buffer.splice(pos, 0, entry)

    if (this.mode === 'audio') {
      this.dropStaleEntries(AUDIO_STALE_THRESHOLD_MS)
    }

    // remove the oldest element if the buffer is full
    if (this.buffer.length > this.maxBufferSize) {
      console.warn('JitterBuffer is full, removing the oldest element')
      this.buffer.shift()
    }
  }

  private shouldRejectOldData(groupId: bigint, objectId: bigint): boolean {
    if (this.lastPoppedGroupId === null || this.lastPoppedObjectId === null) {
      return false
    }

    // 古いgroupIdは拒否
    if (groupId < this.lastPoppedGroupId) {
      return true
    }

    // 同じgroupIdで古いobjectIdは拒否
    if (groupId === this.lastPoppedGroupId && objectId <= this.lastPoppedObjectId) {
      return true
    }

    return false
  }

  /**
   * @returns the oldest object in the buffer if it is older than the minimum delay, otherwise null.
   */
  pop(): JitterBufferSubgroupObject | null {
    const entry = this.popWithMetadata()
    return entry ? entry.object : null
  }

  /**
   * @returns the oldest entry (including groupId and objectId) in the buffer if it is older than the minimum delay, otherwise null.
   */
  popWithMetadata(): JitterBufferEntry | null {
    if (this.buffer.length === 0) {
      return null
    }

    if (this.mode === 'video_normal' || this.mode === 'audio') {
      return this.popNormalMode()
    } else {
      return this.popCorrectlyMode()
    }
  }

  private popNormalMode(): JitterBufferEntry | null {
    const buffer = this.buffer[0]
    const delayMs = performance.now() - buffer.timestamp

    if (delayMs < this.minDelayMs) {
      return null
    }

    this.buffer.shift()
    this.recordPopResult(buffer)
    return buffer
  }

  private popCorrectlyMode(): JitterBufferEntry | null {
    const expectedEntry = this.getExpectedNextEntry()

    // 期待するエントリを検索
    let index = this.buffer.findIndex(
      (entry) => entry.groupId === expectedEntry.groupId && entry.objectId === expectedEntry.objectId
    )

    // 期待するデータが見つからない場合はnullを返す
    if (index === -1) {
      index = this.findResyncIndex()
      if (index === -1) {
        // デバッグ: バッファの状態をログ出力（初回のみ）
        if (this.lastPoppedGroupId === null && this.buffer.length > 0) {
          const bufferedEntries = this.buffer.slice(0, 5).map((e) => `(g:${e.groupId}, o:${e.objectId})`)
          console.warn(
            `[JitterBuffer] Expected first entry not found. Expected: (g:${expectedEntry.groupId}, o:${expectedEntry.objectId}), Buffer: [${bufferedEntries.join(', ')}]`
          )
        }
        return null
      }
    }

    const buffer = this.buffer[index]
    const delayMs = performance.now() - buffer.timestamp

    // 遅延時間が足りない場合はnullを返す
    if (delayMs < this.minDelayMs) {
      return null
    }

    this.buffer.splice(index, 1)
    this.recordPopResult(buffer)
    return buffer
  }

  private recordPopResult(entry: JitterBufferEntry): void {
    this.lastPoppedGroupId = entry.groupId
    this.lastPoppedObjectId = entry.objectId

    if (entry.isEndOfGroup) {
      this.lastPoppedGroupId = entry.groupId + 1n
      this.lastPoppedObjectId = -1n
    }
  }

  private getExpectedNextEntry(): { groupId: bigint; objectId: bigint } {
    // 初回は必ずgroupId=0, objectId=0（キーフレーム）を待つ
    if (this.lastPoppedGroupId === null || this.lastPoppedObjectId === null) {
      return { groupId: 0n, objectId: 0n }
    }

    // keyframeIntervalが設定されている場合、KEYFRAME_INTERVAL-1に達したら次のgroupIdへ
    if (this.keyframeInterval !== undefined && this.lastPoppedObjectId === this.keyframeInterval - 1n) {
      return { groupId: this.lastPoppedGroupId + 1n, objectId: 0n }
    }

    // 次のobjectIdを返す（groupIdは同じ）
    return { groupId: this.lastPoppedGroupId, objectId: this.lastPoppedObjectId + 1n }
  }

  private findInsertPos(groupId: bigint, objectId: bigint): number {
    // Most of the time, the newest object will be received, so search from the end of the buffer
    for (let i = this.buffer.length - 1; i >= 0; i--) {
      const buffer = this.buffer[i]
      if (buffer.groupId === groupId && buffer.objectId < objectId) {
        return i + 1
      }
      if (buffer.groupId < groupId) {
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

  private dropStaleEntries(maxStaleAgeMs: number): void {
    const now = Date.now()
    while (this.buffer.length > 0) {
      const head = this.buffer[0]
      const reference = head.referenceTime
      if (reference === undefined) {
        break
      }
      if (now - reference <= maxStaleAgeMs) {
        break
      }
      this.buffer.shift()
    }
  }
}

function computeReferenceTime(metadata: ChunkMetadata): number | undefined {
  if (typeof metadata.sentAt === 'number') {
    return metadata.sentAt
  }
  if (typeof metadata.timestamp === 'number') {
    return performance.timeOrigin + metadata.timestamp / 1000
  }
  return undefined
}
