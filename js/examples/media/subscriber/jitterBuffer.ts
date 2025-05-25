const DEFAULT_JITTER_BUFFER_SIZE = 30

type JitterBufferEntry = {
    groupId: number,
    objectId: number,
    timestamp: number,
    object: any,
}

/**
 * JitterBuffer is a class that implements the jitter buffer for SubgroupObjects.
 * It stores objects in a sorted order based on groupId and objectId.
 * When reordering, groupId takes precedence over objectID.
 */
export class JitterBuffer {
    private _buffer: Array<JitterBufferEntry>
    private _min_delay_ms: number
    private _max_buffer_size: number

    constructor(min_delay_ms: number, max_buffer_size?: number) {
        this._buffer = []
        this._min_delay_ms = min_delay_ms
        this._max_buffer_size = max_buffer_size ? max_buffer_size : DEFAULT_JITTER_BUFFER_SIZE
    }

    push(groupId: number, objectId: number, object: any) {
        const timestamp = Date.now()
        const entry: JitterBufferEntry = { groupId, objectId, timestamp, object }

        const insertPosition = this._findInsertPosition(groupId, objectId)
        this._buffer.splice(insertPosition, 0, entry)

        // remove the oldest element if the buffer is full
        if (this._buffer.length > this._max_buffer_size) {
            console.warn('JitterBuffer is full, removing the oldest element')
            this._buffer.shift()
        }   
    }

    /**
     * @returns the oldest object in the buffer if it is older than the minimum delay, otherwise null.
     */
    pop(): any | null {
        if (this._buffer.length === 0) {
            return null
        }

        const buffer = this._buffer[0]
        const delayMs =  Date.now() - buffer.timestamp

        if (delayMs < this._min_delay_ms) {
            return null
        }

        this._buffer.shift()
        return buffer.object
    }

    private _findInsertPosition(groupId: number, objectId: number): number {
        // Most of the time, the newest object will be received, so search from the end of the buffer
        for (let i = this._buffer.length - 1; i >= 0; i--) {
            const buffer = this._buffer[i]
            if (buffer.groupId === groupId && buffer.objectId < objectId) {
                return i + 1
            }
            if (buffer.groupId < groupId) {
                return i + 1
            }
            if (buffer.groupId > groupId) {
                return i
            }
        }
        return 0
    }
}