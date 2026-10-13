import { monotonicUnixMicros } from '../clock'

const MAX_PENDING_FRAMES = 1024

/// An encoder output carries the timestamp of the input it was encoded from,
/// which is how its wall-clock capture time is found again. Outputs are
/// restamped from the first one so that every stream starts at zero.
export class CaptureTimestamps {
  private readonly captureMicrosByTimestamp = new Map<number, number>()
  private firstTimestamp: number | undefined

  reset(): void {
    this.captureMicrosByTimestamp.clear()
    this.firstTimestamp = undefined
  }

  recordCapture(inputTimestamp: number): void {
    this.captureMicrosByTimestamp.set(inputTimestamp, monotonicUnixMicros())
    if (this.captureMicrosByTimestamp.size > MAX_PENDING_FRAMES) {
      this.captureMicrosByTimestamp.delete(this.captureMicrosByTimestamp.keys().next().value!)
    }
  }

  restamp(chunk: EncodedAudioChunk | EncodedVideoChunk): {
    init: EncodedVideoChunkInit
    captureTimestampMicros: number | undefined
  } {
    this.firstTimestamp ??= chunk.timestamp
    const data = new ArrayBuffer(chunk.byteLength)
    chunk.copyTo(data)
    const captureTimestampMicros = this.captureMicrosByTimestamp.get(chunk.timestamp)
    this.captureMicrosByTimestamp.delete(chunk.timestamp)
    return {
      init: {
        type: chunk.type,
        timestamp: chunk.timestamp - this.firstTimestamp,
        duration: chunk.duration ?? undefined,
        data
      },
      captureTimestampMicros
    }
  }
}
