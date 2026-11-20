/// Picks the keyframes of a video by time, so a keyframe interval means the
/// same in every rendition whatever the frame rate.
export class KeyframeSchedule {
  private lastKeyframeMicros: number | undefined

  constructor(public intervalMicros: number) {}

  /// The next frame is a keyframe.
  reset(): void {
    this.lastKeyframeMicros = undefined
  }

  /// Records the frame as the last keyframe when it returns true, so call it
  /// once per frame in timestamp order.
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
