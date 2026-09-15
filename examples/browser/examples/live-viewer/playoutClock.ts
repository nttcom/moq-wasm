const MICROS_PER_MILLI = 1_000

export type PlayoutTime = {
  atMs: number
  /// The clock moved onto this sample, so whatever was scheduled before it
  /// belongs to a timeline that no longer applies.
  reanchored: boolean
}

/// Maps capture timestamps onto the local clock so that audio and video
/// captured at the same instant are presented at the same instant. `delayMs`
/// after a sample arrives is when it is due once the clock is anchored on it,
/// and that delay is the jitter budget. A master sample that misses its time
/// is presented at once and pushes the clock back by the miss so the samples
/// behind it stay contiguous, and a master sample due more than `maxEarlyMs`
/// past the budget re-anchors so the extra latency it reveals is shed. Samples
/// that are not the master are placed on the clock as it stands.
export class PlayoutClock {
  private origin: { captureMicros: number; atMs: number } | undefined

  constructor(
    private readonly delayMs: number,
    private readonly maxEarlyMs: number
  ) {}

  get anchored(): boolean {
    return this.origin !== undefined
  }

  anchor(captureMicros: number, nowMs: number): void {
    this.origin = { captureMicros, atMs: nowMs + this.delayMs }
  }

  dueAt(captureMicros: number): number | undefined {
    if (!this.origin) {
      return undefined
    }
    return this.origin.atMs + (captureMicros - this.origin.captureMicros) / MICROS_PER_MILLI
  }

  playoutTime(captureMicros: number, nowMs: number, master: boolean): PlayoutTime {
    const due = this.dueAt(captureMicros)
    if (due === undefined) {
      this.anchor(captureMicros, nowMs)
      return { atMs: nowMs + this.delayMs, reanchored: false }
    }
    const lead = due - nowMs
    if (master && lead > this.delayMs + this.maxEarlyMs) {
      this.anchor(captureMicros, nowMs)
      return { atMs: nowMs + this.delayMs, reanchored: true }
    }
    if (lead < 0) {
      if (master) {
        this.shift(-lead)
      }
      return { atMs: nowMs, reanchored: false }
    }
    return { atMs: due, reanchored: false }
  }

  /// Moves every playout time by `deltaMs`; the audio playout reports how far
  /// the device clock has run from this one.
  shift(deltaMs: number): void {
    if (this.origin) {
      this.origin = { captureMicros: this.origin.captureMicros, atMs: this.origin.atMs + deltaMs }
    }
  }

  reset(): void {
    this.origin = undefined
  }
}
