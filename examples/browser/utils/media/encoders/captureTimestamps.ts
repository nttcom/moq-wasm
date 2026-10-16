import { monotonicUnixMicros } from '../clock'

const MAX_PENDING_FRAMES = 1024
/// Chrome's Opus encoder stamps outputs from a rounded sample count, so an
/// output can sit a microsecond or two off the input it was encoded from.
/// Half the 10 ms spacing of the capture inputs keeps a match unambiguous.
const MATCH_TOLERANCE_MICROS = 5_000

/// An encoder output carries the timestamp of the input it was encoded from,
/// which is how its wall-clock capture time is found again. Outputs are
/// restamped from the first one so that every stream starts at zero.
export class CaptureTimestamps {
  private readonly captureMicrosByTimestamp = new Map<number, number>()
  private firstTimestamp: number | undefined

  constructor(private readonly now: () => number = monotonicUnixMicros) {}

  reset(): void {
    this.captureMicrosByTimestamp.clear()
    this.firstTimestamp = undefined
  }

  recordCapture(inputTimestamp: number): void {
    this.captureMicrosByTimestamp.set(inputTimestamp, this.now())
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
    return {
      init: {
        type: chunk.type,
        timestamp: chunk.timestamp - this.firstTimestamp,
        duration: chunk.duration ?? undefined,
        data
      },
      captureTimestampMicros: this.takeCaptureMicros(chunk.timestamp)
    }
  }

  private takeCaptureMicros(timestamp: number): number | undefined {
    let matched: number | undefined
    let matchedDistance = MATCH_TOLERANCE_MICROS
    for (const inputTimestamp of this.captureMicrosByTimestamp.keys()) {
      const distance = Math.abs(inputTimestamp - timestamp)
      if (distance <= matchedDistance) {
        matched = inputTimestamp
        matchedDistance = distance
      }
    }
    if (matched === undefined) {
      return undefined
    }
    const captureMicros = this.captureMicrosByTimestamp.get(matched)
    this.captureMicrosByTimestamp.delete(matched)
    return captureMicros
  }
}
