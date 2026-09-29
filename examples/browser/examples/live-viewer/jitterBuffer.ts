const WINDOW_MS = 10_000
const MICROS_PER_MILLI = 1_000

export type BufferPolicy =
  | { mode: 'adaptive'; minimumMs: number; extraMs: number }
  | { mode: 'fixed'; bufferMs: number }

type Arrival = { atMs: number; delayMs: number }

/// Sizes the live playout buffer. The delay from capture to arrival of every
/// sample of the last `WINDOW_MS` is kept; each delay carries the offset
/// between the capture clock and the local one, which only the differences
/// between them cancel. An adaptive buffer covers the spread between the
/// fastest and the slowest of them, never less than its minimum, plus a fixed
/// extra delay for swings wider than the window has seen. A fixed buffer
/// ignores the spread.
export class JitterBuffer {
  private readonly arrivals: Arrival[] = []
  private observingSinceMs: number | undefined

  constructor(private policy: BufferPolicy) {}

  get currentPolicy(): BufferPolicy {
    return this.policy
  }

  setPolicy(policy: BufferPolicy): void {
    this.policy = policy
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
    return this.policy.mode === 'fixed' || newest.atMs - this.observingSinceMs >= WINDOW_MS
  }

  fastestDelayMs(): number | undefined {
    return this.arrivals.length === 0 ? undefined : Math.min(...this.arrivals.map((arrival) => arrival.delayMs))
  }

  targetMs(): number {
    return this.targetFor(this.spreadMs())
  }

  /// The target to open playback with, when the burst a subscription starts
  /// with says nothing about the spread and `observedSpreadMs` stands in for it.
  openingTargetMs(observedSpreadMs: number): number {
    return this.targetFor(observedSpreadMs)
  }

  reset(): void {
    this.arrivals.length = 0
    this.observingSinceMs = undefined
  }

  private targetFor(spreadMs: number): number {
    const policy = this.policy
    return policy.mode === 'fixed' ? policy.bufferMs : Math.max(policy.minimumMs, spreadMs) + policy.extraMs
  }

  private spreadMs(): number {
    const fastest = this.fastestDelayMs()
    if (fastest === undefined) {
      return 0
    }
    return Math.max(...this.arrivals.map((arrival) => arrival.delayMs)) - fastest
  }
}
