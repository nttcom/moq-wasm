const WINDOW_MS = 10_000
const MICROS_PER_MILLI = 1_000

export type BufferPolicy = { minimumMs: number; maximumMs: number }

type Arrival = { atMs: number; delayMs: number }

/// Sizes the live playout buffer. The delay from capture to arrival of every
/// sample of the last `WINDOW_MS` is kept; each delay carries the offset
/// between the capture clock and the local one, which only the differences
/// between them cancel. The buffer covers the spread between the fastest and
/// the slowest of them plus the time a sample takes from being handed over to
/// being heard, and the policy clamps that sum; equal bounds fix it.
export class JitterBuffer {
  private readonly arrivals: Arrival[] = []
  private observingSinceMs: number | undefined

  constructor(public policy: BufferPolicy) {}

  get fixed(): boolean {
    return this.policy.minimumMs >= this.policy.maximumMs
  }

  observe(captureMicros: number, arrivedAtMs: number): void {
    this.observingSinceMs ??= arrivedAtMs
    this.arrivals.push({ atMs: arrivedAtMs, delayMs: arrivedAtMs - captureMicros / MICROS_PER_MILLI })
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

  fastestDelayMs(): number | undefined {
    return this.arrivals.length === 0 ? undefined : Math.min(...this.arrivals.map((arrival) => arrival.delayMs))
  }

  /// Playback opens on a burst that says nothing about the spread, so the
  /// caller passes a stand-in for it then.
  targetMs(outputLatencyMs: number, spreadMs = this.spreadMs() ?? 0): number {
    const { minimumMs, maximumMs } = this.policy
    return Math.max(minimumMs, Math.min(maximumMs, spreadMs + outputLatencyMs))
  }

  spreadMs(): number | undefined {
    const fastest = this.fastestDelayMs()
    if (fastest === undefined) {
      return undefined
    }
    return Math.max(...this.arrivals.map((arrival) => arrival.delayMs)) - fastest
  }

  reset(): void {
    this.arrivals.length = 0
    this.observingSinceMs = undefined
  }
}
