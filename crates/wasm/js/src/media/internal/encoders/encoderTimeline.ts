/// The encoder stamps its first chunk 0, but that chunk's frame can be
/// captured seconds after the encoder started, so the first chunk's capture
/// time anchors the chunks, not the encoder's start.
export class EncoderTimeline {
  private offsetMicros: number | undefined

  constructor(
    private readonly publicationStartMicros: number,
    private readonly fallbackOffsetMicros: number
  ) {}

  /// Microseconds since the publication began. `captureMicros` is on the
  /// clock of `publicationStartMicros`.
  place(chunkTimestampMicros: number, captureMicros: number | undefined): number {
    this.offsetMicros ??=
      captureMicros === undefined
        ? this.fallbackOffsetMicros
        : captureMicros - this.publicationStartMicros - chunkTimestampMicros
    return chunkTimestampMicros + this.offsetMicros
  }
}
