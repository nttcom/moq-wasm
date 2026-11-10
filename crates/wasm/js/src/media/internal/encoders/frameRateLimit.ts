/// Thins a video's frames to at most `framerate` per second, by their timestamps.
///
/// A frame up to `EARLY_TOLERANCE` of an interval early still passes, so a
/// source at the limit keeps every frame despite capture jitter, while a source
/// at twice the limit still loses every other frame. A gap of two intervals or
/// more, such as a still screen share, restarts the count from the next frame.
const EARLY_TOLERANCE = 0.4

export class FrameRateLimit {
  private readonly intervalMicros: number
  private nextDueMicros: number | undefined

  constructor(framerate: number) {
    this.intervalMicros = 1_000_000 / framerate
  }

  /// Whether the frame at `timestampMicros` is encoded. A `keyframe` always
  /// is, so every rendition keeps the frames its groups start on.
  admit(timestampMicros: number, keyframe: boolean): boolean {
    const due = this.nextDueMicros
    if (keyframe || due === undefined || Math.abs(timestampMicros - due) >= 2 * this.intervalMicros) {
      this.nextDueMicros = timestampMicros + this.intervalMicros
      return true
    }
    if (timestampMicros < due - EARLY_TOLERANCE * this.intervalMicros) {
      return false
    }
    this.nextDueMicros = due + this.intervalMicros
    return true
  }
}
