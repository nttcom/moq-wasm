export interface WarmableEncoder {
  /// Encodes `frame` synchronously, before the caller closes it, and settles
  /// once the encoder has emitted (and discarded) its output.
  warmUp(frame: VideoFrame): Promise<void>
}

/// Drops live frames until every new encoder has flushed one warm-up frame.
///
/// Chrome's first encoder of a process takes seconds to become ready on a Mac.
/// Meanwhile the camera stops capturing, and the frames captured just before
/// arrive together once the encoder is ready; encoded, they would go out
/// seconds late as if live. The warm-up waits on `flush()`, not on an output,
/// because an encoder that looks ahead (latencyMode 'quality') may hold the
/// output of a single frame until it is flushed.
export class EncoderWarmUp {
  private warming: Promise<unknown> | undefined
  private warm = false

  constructor(private readonly onFailed: (error: unknown) => void) {}

  /// Call after creating new encoders: the next frame passed to `admit` warms them up.
  reset(): void {
    this.warming = undefined
    this.warm = false
  }

  /// Whether `frame` goes to the encoders as a live frame. False for the first
  /// frame after `reset`, which warms them up, and for the frames read until the
  /// warm-up settles; the caller closes those.
  admit(frame: VideoFrame, encoders: WarmableEncoder[]): boolean {
    if (this.warm) {
      return true
    }
    if (!this.warming) {
      const warming = Promise.all(encoders.map((encoder) => encoder.warmUp(frame)))
      this.warming = warming
      warming.then(
        () => this.finish(warming),
        (error) => {
          if (this.finish(warming)) {
            this.onFailed(error)
          }
        }
      )
    }
    return false
  }

  /// Marks the encoders warm unless a `reset` replaced `warming`; returns whether it did.
  private finish(warming: Promise<unknown>): boolean {
    if (this.warming !== warming) {
      return false
    }
    this.warm = true
    return true
  }
}
