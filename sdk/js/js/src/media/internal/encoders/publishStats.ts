/// What one encoder output over the last second.
export type EncodedRateStats = {
  encodedKbps: number
  encodedFramesPerSecond: number
}

export type EncodedVideoStats = EncodedRateStats & {
  /// The coded size of the latest decoder configuration the encoder output;
  /// undefined until it outputs one.
  frameSize: { width: number; height: number } | undefined
}

export type VideoPublishStats = EncodedVideoStats & {
  /// Frames the encoder worker closed without encoding: an encoder's queue was
  /// full, or no encoder could be configured.
  droppedFrames: number
  /// Encoder configuration and encode failures, and encoder worker failures.
  encoderErrors: number
  /// By rendition name.
  renditions: Record<string, EncodedVideoStats>
}

export type AudioPublishStats = EncodedRateStats & {
  encoderErrors: number
}

/// A kind is undefined while the source has no track of it. Counts are since
/// the source was published, across mutes, replaced tracks and encoder restarts.
export type SourcePublicationStats = {
  video: VideoPublishStats | undefined
  audio: AudioPublishStats | undefined
}

const RATE_WINDOW_MS = 1000

class EncodedRate {
  private readonly chunks: { atMs: number; byteLength: number }[] = []

  add(byteLength: number, nowMs: number): void {
    this.chunks.push({ atMs: nowMs, byteLength })
    this.prune(nowMs)
  }

  stats(nowMs: number): EncodedRateStats {
    this.prune(nowMs)
    const bytes = this.chunks.reduce((sum, chunk) => sum + chunk.byteLength, 0)
    return {
      encodedKbps: (bytes * 8) / RATE_WINDOW_MS,
      encodedFramesPerSecond: (this.chunks.length * 1000) / RATE_WINDOW_MS
    }
  }

  private prune(nowMs: number): void {
    while (this.chunks.length > 0 && this.chunks[0].atMs <= nowMs - RATE_WINDOW_MS) {
      this.chunks.shift()
    }
  }
}

class EncodedVideo {
  private readonly rate = new EncodedRate()
  private frameSize: { width: number; height: number } | undefined

  add(chunk: EncodedVideoChunk, metadata: EncodedVideoChunkMetadata | undefined, nowMs: number): void {
    this.rate.add(chunk.byteLength, nowMs)
    const config = metadata?.decoderConfig
    if (config?.codedWidth && config.codedHeight) {
      this.frameSize = { width: config.codedWidth, height: config.codedHeight }
    }
  }

  stats(nowMs: number): EncodedVideoStats {
    return { ...this.rate.stats(nowMs), frameSize: this.frameSize }
  }
}

/// The encoder worker numbers the video 0 and its renditions from 1, in the
/// order of `renditionNames`.
export class VideoEncodeStats {
  droppedFrames = 0
  encoderErrors = 0
  private readonly encoders: EncodedVideo[]

  constructor(private readonly renditionNames: string[]) {
    this.encoders = [new EncodedVideo(), ...renditionNames.map(() => new EncodedVideo())]
  }

  add(
    rendition: number,
    chunk: EncodedVideoChunk,
    metadata: EncodedVideoChunkMetadata | undefined,
    nowMs = performance.now()
  ): void {
    this.encoders[rendition].add(chunk, metadata, nowMs)
  }

  stats(nowMs = performance.now()): VideoPublishStats {
    return {
      ...this.encoders[0].stats(nowMs),
      droppedFrames: this.droppedFrames,
      encoderErrors: this.encoderErrors,
      renditions: Object.fromEntries(
        this.renditionNames.map((name, index) => [name, this.encoders[index + 1].stats(nowMs)])
      )
    }
  }
}

export class AudioEncodeStats {
  encoderErrors = 0
  private readonly rate = new EncodedRate()

  add(chunk: EncodedAudioChunk, nowMs = performance.now()): void {
    this.rate.add(chunk.byteLength, nowMs)
  }

  stats(nowMs = performance.now()): AudioPublishStats {
    return { ...this.rate.stats(nowMs), encoderErrors: this.encoderErrors }
  }
}
