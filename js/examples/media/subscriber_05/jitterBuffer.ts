const DEFAULT_MIN_DELAY_MS = 2 * 50
const DEFAULT_JITTER_BUFFER_SIZE = 8 * 1800

type JitterBufferEntry<T> = {
  groupId: number
  objectId: number
  timestamp: number
  object: T
}

/**
 * JitterBuffer is a class that implements the jitter buffer for SubgroupObjects.
 * It stores objects in a sorted order based on groupId and objectId.
 * When reordering, groupId takes precedence over objectID.
 */
export class JitterBuffer<T> {
  private buffer: JitterBufferEntry<T>[] = []
  private minDelayMs: number = DEFAULT_MIN_DELAY_MS

  constructor(private readonly maxBufferSize: number = DEFAULT_JITTER_BUFFER_SIZE) {}

  push(groupId: number, objectId: number, object: T) {
    const timestamp = performance.now()
    const entry = { groupId, objectId, timestamp, object }

    const pos = this.findInsertPos(groupId, objectId)
    this.buffer.splice(pos, 0, entry)

    // remove the oldest element if the buffer is full
    if (this.buffer.length > this.maxBufferSize) {
      console.warn('JitterBuffer is full, removing the oldest element')
      this.buffer.shift()
    }
  }

  /**
   * @returns the oldest object in the buffer if it is older than the minimum delay, otherwise null.
   */
  pop(): T | null {
    if (this.buffer.length === 0) {
      return null
    }

    const buffer = this.buffer[0]
    const delayMs = performance.now() - buffer.timestamp

    if (delayMs < this.minDelayMs) {
      return null
    }

    this.buffer.shift()
    return buffer.object
  }

  private findInsertPos(groupId: number, objectId: number): number {
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
}
