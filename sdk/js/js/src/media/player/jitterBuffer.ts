import type { MediaKind } from './trackContext'

const WINDOW_MS = 10_000
const MICROS_PER_MILLI = 1_000

/// `arrivalQuantile` is the share of the window's arrivals the buffer is in
/// time for: 1, the default, covers the slowest of them, and a lower one
/// leaves the rarest delays to the audio concealment for less latency.
/// While audio plays, a `maximumMs` below what the audio output needs is
/// raised to it. From a chunk's arrival the output needs its latency, the
/// 20 ms a chunk is scheduled ahead of that, and one more chunk. With less,
/// every chunk would miss its time.
export type BufferPolicy = { minimumMs: number; maximumMs: number; arrivalQuantile?: number }

type Arrival = { atMs: number; delayMs: number; kind: MediaKind }

/// Sizes the live playout buffer. The delay from capture to arrival of every
/// sample of the last `WINDOW_MS` is kept; each delay carries the offset
/// between the capture clock and the local one, which only the differences
/// between them cancel. The buffer covers the spread between the fastest of
/// them and the policy's quantile of them plus the time a sample takes from
/// being handed over to being heard, and the policy clamps that sum; equal
/// bounds fix it.
export class JitterBuffer {
  private readonly arrivals: Arrival[] = []
  private observingSinceMs: number | undefined

  constructor(public policy: BufferPolicy) {}

  get fixed(): boolean {
    return this.policy.minimumMs >= this.policy.maximumMs
  }

  observe(kind: MediaKind, captureMicros: number, arrivedAtMs: number): void {
    this.observingSinceMs ??= arrivedAtMs
    this.arrivals.push({ atMs: arrivedAtMs, delayMs: arrivedAtMs - captureMicros / MICROS_PER_MILLI, kind })
    while (this.arrivals[0].atMs < arrivedAtMs - WINDOW_MS) {
      this.arrivals.shift()
    }
  }

  /// Until a whole window has been observed, a spread that looks small may
  /// only mean the slowest arrivals have not come yet.
  get settled(): boolean {
    const newest = this.arrivals.at(-1)
    if (newest === undefined || this.observingSinceMs === undefined) {
      return false
    }
    return this.fixed || newest.atMs - this.observingSinceMs >= WINDOW_MS
  }

  fastestDelayMs(kind?: MediaKind): number | undefined {
    const arrivals = kind === undefined ? this.arrivals : this.arrivals.filter((arrival) => arrival.kind === kind)
    return arrivals.length === 0 ? undefined : Math.min(...arrivals.map((arrival) => arrival.delayMs))
  }

  medianDelayMs(kind: MediaKind): number | undefined {
    const delaysMs = this.arrivals
      .filter((arrival) => arrival.kind === kind)
      .map((arrival) => arrival.delayMs)
      .sort((left, right) => left - right)
    return delaysMs.length === 0 ? undefined : delaysMs[Math.floor(delaysMs.length / 2)]
  }

  /// Playback opens before any spread has been observed, so the caller
  /// passes a stand-in for it then. `outputFloorMs` is the least buffer the
  /// audio output plays without running out; it overrides a smaller maximum.
  targetMs(outputLatencyMs: number, outputFloorMs: number, spreadMs = this.spreadMs() ?? 0): number {
    const maximumMs = Math.max(this.policy.maximumMs, outputFloorMs)
    return Math.max(this.policy.minimumMs, Math.min(maximumMs, spreadMs + outputLatencyMs))
  }

  spreadMs(): number | undefined {
    const fastest = this.fastestDelayMs()
    if (fastest === undefined) {
      return undefined
    }
    const delaysMs = this.arrivals.map((arrival) => arrival.delayMs).sort((left, right) => left - right)
    const index = Math.ceil((this.policy.arrivalQuantile ?? 1) * delaysMs.length) - 1
    return delaysMs[Math.max(0, index)] - fastest
  }

  reset(): void {
    this.arrivals.length = 0
    this.observingSinceMs = undefined
  }
}
