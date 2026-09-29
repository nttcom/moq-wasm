const MICROS_PER_MILLI = 1_000

/// Maps capture timestamps onto the local clock so that audio and video
/// captured at the same instant are presented at the same instant.
export type PlayoutOrigin = { captureMicros: number; atMs: number }

export class PlayoutClock {
  private origin: PlayoutOrigin | undefined
  private originListener: ((origin: PlayoutOrigin | undefined) => void) | undefined

  /// Called whenever the mapping from capture time to local time changes.
  setOriginListener(listener: (origin: PlayoutOrigin | undefined) => void): void {
    this.originListener = listener
  }

  private originChanged(): void {
    this.originListener?.(this.origin && { ...this.origin })
  }

  get anchored(): boolean {
    return this.origin !== undefined
  }

  /// How long after its capture a sample is presented, measured with the
  /// capture timestamp in local milliseconds.
  get offsetMs(): number | undefined {
    return this.origin && this.origin.atMs - this.origin.captureMicros / MICROS_PER_MILLI
  }

  anchor(captureMicros: number, atMs: number): void {
    this.origin = { captureMicros, atMs }
    this.originChanged()
  }

  dueAt(captureMicros: number): number | undefined {
    if (!this.origin) {
      return undefined
    }
    return this.origin.atMs + (captureMicros - this.origin.captureMicros) / MICROS_PER_MILLI
  }

  /// Moves every playout time by `deltaMs`; the audio playout reports how far
  /// the device clock has run from this one.
  shift(deltaMs: number): void {
    if (this.origin) {
      this.origin = { ...this.origin, atMs: this.origin.atMs + deltaMs }
      this.originChanged()
    }
  }

  reset(): void {
    this.origin = undefined
    this.originChanged()
  }
}
