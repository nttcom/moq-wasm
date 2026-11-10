/// Picks the keyframes of a video by time, so a keyframe interval means the
/// same in every rendition whatever the frame rate.
export class KeyframeSchedule {
  private lastKeyframeMicros: number | undefined

  constructor(public intervalMicros: number) {}

  /// The next frame is a keyframe.
  reset(): void {
    this.lastKeyframeMicros = undefined
  }

  /// Whether the frame at `timestampMicros` is a keyframe: the first frame,
  /// a `forced` one, or the first `intervalMicros` or more after the last keyframe.
  isKeyframe(timestampMicros: number, forced: boolean): boolean {
    const keyframe =
      forced ||
      this.lastKeyframeMicros === undefined ||
      timestampMicros - this.lastKeyframeMicros >= this.intervalMicros
    if (keyframe) {
      this.lastKeyframeMicros = timestampMicros
    }
    return keyframe
  }
}
