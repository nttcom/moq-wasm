const MICROS_PER_SECOND = 1_000_000

/// Where Chrome's audio encoder places an input on its output timeline. An
/// output is stamped with the timestamp of the first input since `configure`
/// plus the duration of the samples encoded since, ignoring the timestamps of
/// later inputs. A gap in the input timestamps leaves every later output that
/// far from its input's timestamp; keyed by this position, a capture time
/// still finds its output.
export class EncoderSamplePosition {
  private nextMicros: number | undefined

  /// Call after every `configure`: the encoder restarts its timeline from the
  /// first input that follows.
  reset(): void {
    this.nextMicros = undefined
  }

  /// Call once per input, in encode order: each position follows the previous
  /// input's samples.
  take(input: Pick<AudioData, 'timestamp' | 'numberOfFrames' | 'sampleRate'>): number {
    const position = this.nextMicros ?? input.timestamp
    this.nextMicros = position + (input.numberOfFrames * MICROS_PER_SECOND) / input.sampleRate
    return Math.round(position)
  }
}
