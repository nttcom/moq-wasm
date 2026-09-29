const WINDOW_MS = 10_000
const MICROS_PER_MILLI = 1_000

export type BufferPolicy = { minimumMs: number; maximumMs: number }

type Arrival = { atMs: number; delayMs: number }

/// Sizes the live playout buffer. The delay from capture to arrival of every
/// sample of the last `WINDOW_MS` is kept; each delay carries the offset
/// between the capture clock and the local one, which only the differences
/// between them cancel. The buffer covers the spread between the fastest and
/// the slowest of them, clamped to the policy; equal bounds fix it.
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
    return Math.max(this.policy.minimumMs, Math.min(this.policy.maximumMs, spreadMs))
  }

  private spreadMs(): number {
    const fastest = this.fastestDelayMs()
    if (fastest === undefined) {
      return 0
    }
    return Math.max(...this.arrivals.map((arrival) => arrival.delayMs)) - fastest
  }
}
