import type { JitterBufferSubgroupObject } from './jitterBufferTypes'

const MAX_ENTRIES = 9000

export type VideoJitterBufferEntry = {
  groupId: bigint
  captureTimestampMicros?: number
  receivedAtMs: number
  object: JitterBufferSubgroupObject
}

export class VideoJitterBuffer {
  private buffer: VideoJitterBufferEntry[] = []

  push(entry: VideoJitterBufferEntry): void {
    this.buffer.splice(this.findInsertPos(entry), 0, entry)
    if (this.buffer.length > MAX_ENTRIES) {
      this.buffer.shift()
    }
  }

  pop(): VideoJitterBufferEntry | null {
    return this.buffer.shift() ?? null
  }

  peek(): VideoJitterBufferEntry | null {
    return this.buffer[0] ?? null
  }

  private findInsertPos({ groupId, object }: VideoJitterBufferEntry): number {
    const subgroupId = object.subgroupId ?? 0n
    for (let i = this.buffer.length - 1; i >= 0; i -= 1) {
      const entry = this.buffer[i]
      const entryObject = entry.object
      if (entry.groupId === groupId && entryObject.objectId < object.objectId) {
        return i + 1
      }
      if (
        entry.groupId === groupId &&
        entryObject.objectId === object.objectId &&
        (entryObject.subgroupId ?? 0n) <= subgroupId
      ) {
        return i + 1
      }
      if (entry.groupId < groupId) {
        return i + 1
      }
    }
    return 0
  }
}
