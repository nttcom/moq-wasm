const MAX_ENTRIES = 9000

export type VideoJitterBufferEntry = {
  groupId: bigint
  subgroupId: bigint
  objectId: bigint
  captureTimestampMicros?: number
  receivedAtMs: number
  videoConfig?: Uint8Array
  chunk: EncodedVideoChunk
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

  private findInsertPos({ groupId, objectId, subgroupId }: VideoJitterBufferEntry): number {
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
}
