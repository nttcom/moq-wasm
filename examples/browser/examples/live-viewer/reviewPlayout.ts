import { AudioPlayout } from './audioPlayout'
import { PlayoutClock } from './playoutClock'
import { VideoPlayout } from './videoPlayout'
import type { ReviewFrame } from './rewind'

const REVIEW_LEAD_MS = 150
const MICROS_PER_MILLI = 1_000
const WAIT_POLL_MS = 200

/// Presents a review. The fetched media is decoded ahead of time, so the clock
/// only maps capture timestamps onto local time from the position the review
/// starts at, and the drift the audio device shows moves it so the picture
/// keeps step with the sound. Pausing freezes both, and resuming moves the
/// clock by the pause so playback carries on where it stopped.
export class ReviewPlayout {
  private readonly clock = new PlayoutClock(REVIEW_LEAD_MS, Number.POSITIVE_INFINITY)
  private readonly audio = new AudioPlayout((driftMs) => this.clock.shift(driftMs))
  private readonly video: VideoPlayout
  private audioDecoder: AudioDecoder | undefined
  private shownFromMicros = 0
  private pausedAtMs: number | undefined

  constructor(
    show: (frame: VideoFrame) => void,
    private readonly onError: (message: string) => void
  ) {
    this.video = new VideoPlayout(show)
  }

  start(shownFromMicros: number): void {
    this.stop()
    this.shownFromMicros = shownFromMicros
    this.clock.anchor(shownFromMicros, performance.now())
  }

  stop(): void {
    this.video.flush()
    this.video.setPaused(false)
    this.audio.flush()
    void this.audio.resume()
    this.audioDecoder?.close()
    this.audioDecoder = undefined
    this.clock.reset()
    this.pausedAtMs = undefined
  }

  /// Frames before the position are decoded for the ones after them and
  /// closed unseen.
  presentVideo(frame: VideoFrame): void {
    if (frame.timestamp < this.shownFromMicros) {
      frame.close()
      return
    }
    this.video.present(frame, this.dueAt(frame.timestamp))
  }

  get queuedVideo(): number {
    return this.video.queued
  }

  /// Every chunk is decoded, because an AAC frame needs the one before it,
  /// and the output that ends before the position is dropped.
  decodeAudio(chunks: ReviewFrame[], config: AudioDecoderConfig): void {
    if (chunks.length === 0) {
      return
    }
    this.audio.prepare(config.sampleRate)
    this.audioDecoder ??= this.createAudioDecoder(config)
    for (const chunk of chunks) {
      if (chunk.captureMicros === undefined) {
        continue
      }
      this.audioDecoder.decode(new EncodedAudioChunk({ type: 'key', timestamp: chunk.captureMicros, data: chunk.data }))
    }
  }

  setVolume(volume: number): void {
    this.audio.setVolume(volume)
  }

  setPaused(paused: boolean): void {
    if (paused === (this.pausedAtMs !== undefined)) {
      return
    }
    if (paused) {
      this.pausedAtMs = performance.now()
      this.video.setPaused(true)
      void this.audio.suspend()
      return
    }
    const pausedForMs = performance.now() - (this.pausedAtMs ?? 0)
    this.pausedAtMs = undefined
    this.clock.shift(pausedForMs)
    this.video.shift(pausedForMs)
    this.video.setPaused(false)
    void this.audio.resume()
  }

  /// Resolves once the sample captured at `captureMicros` is due, pausing
  /// with playback; `isCurrent` lets a superseded review stop waiting.
  async waitUntilDue(captureMicros: number, isCurrent: () => boolean): Promise<void> {
    while (isCurrent()) {
      const remainingMs = this.dueAt(captureMicros) - performance.now()
      if (remainingMs <= 0 && this.pausedAtMs === undefined) {
        return
      }
      await new Promise((resolve) => setTimeout(resolve, Math.min(Math.max(remainingMs, 20), WAIT_POLL_MS)))
    }
  }

  /// How far the picture on screen is ahead of the sound.
  syncOffsetMs(): number | undefined {
    const nowMs = performance.now()
    const video = this.video.positionMicrosAt(nowMs)
    const audio = this.audio.positionMicrosAt(nowMs)
    if (video === undefined || audio === undefined) {
      return undefined
    }
    return (video - audio) / MICROS_PER_MILLI
  }

  private dueAt(captureMicros: number): number {
    return this.clock.dueAt(captureMicros) ?? performance.now()
  }

  private createAudioDecoder(config: AudioDecoderConfig): AudioDecoder {
    const decoder = new AudioDecoder({
      output: (audioData) => {
        if (audioData.timestamp + (audioData.duration ?? 0) <= this.shownFromMicros) {
          audioData.close()
          return
        }
        const captureMicros = audioData.timestamp
        this.audio.play(audioData, captureMicros, () => this.dueAt(captureMicros))
        audioData.close()
      },
      error: (error) => this.onError(`review audio decoder: ${error.message}`)
    })
    decoder.configure(config)
    return decoder
  }
}
