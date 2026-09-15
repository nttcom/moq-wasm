const MICROS_PER_MILLI = 1_000

export type PlayoutTime = {
  atMs: number
  /// The anchor moved onto this sample, so whatever was scheduled before it
  /// belongs to a timeline that no longer applies.
  reanchored: boolean
}

/// Maps capture timestamps onto the local clock so that audio and video
/// captured at the same instant are presented at the same instant. The first
/// sample is anchored `delayMs` after it arrives and that delay is the jitter
/// budget: a sample that misses its time is presented at once and pushes the
/// anchor back by the miss so the samples behind it stay contiguous, and a
/// sample due more than `maxEarlyMs` past the budget re-anchors so the extra
/// latency it reveals is shed instead of kept.
export class PlayoutClock {
  private anchor: { captureMicros: number; atMs: number } | undefined

  constructor(
    private readonly delayMs: number,
    private readonly maxEarlyMs: number
  ) {}

  playoutTime(captureMicros: number, nowMs: number): PlayoutTime {
    if (!this.anchor) {
      this.anchor = { captureMicros, atMs: nowMs + this.delayMs }
      return { atMs: this.anchor.atMs, reanchored: false }
    }
    const atMs = this.anchor.atMs + (captureMicros - this.anchor.captureMicros) / MICROS_PER_MILLI
    const lead = atMs - nowMs
    if (lead > this.delayMs + this.maxEarlyMs) {
      this.anchor = { captureMicros, atMs: nowMs + this.delayMs }
      return { atMs: this.anchor.atMs, reanchored: true }
    }
    if (lead < 0) {
      this.anchor = { captureMicros: this.anchor.captureMicros, atMs: this.anchor.atMs - lead }
      return { atMs: nowMs, reanchored: false }
    }
    return { atMs, reanchored: false }
  }

  reset(): void {
    this.anchor = undefined
  }
}
