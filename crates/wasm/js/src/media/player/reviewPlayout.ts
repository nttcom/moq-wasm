import { alawToAudioData, isPcmAlawCodec } from '../internal/alaw'
import { AudioPlayout } from './audioPlayout'
import { syncOffsetMs } from './playoutClock'
import type { ClockFollower, ReviewClock } from './reviewClock'
import { VideoPlayout } from './videoPlayout'
import type { ReviewFrame } from './rewind'

const WAIT_POLL_MS = 200

/// Presents a review on a `ReviewClock`. The fetched media is decoded ahead
/// of time and presented when the clock says it is due. The sound plays
/// through the clock's AudioContext and reports its drift to the clock. A
/// review started on a paused clock shows the frame at its position and holds
/// there.
export class ReviewPlayout implements ClockFollower {
  private readonly audio = new AudioPlayout(
    (driftMs) => this.clock.drift(this, driftMs),
    (sampleRate) => this.clock.audioContext(sampleRate)
  )
  private readonly video: VideoPlayout
  private audioDecoder: AudioDecoder | undefined
  /// The decoder stamps its outputs from the sample count it has produced, so
  /// each output is labelled with the capture timestamp of the chunk it was
  /// decoded from.
  private readonly pendingCaptureMicros: number[] = []
  private shownFromMicros = 0

  /// `clock` is the one the playout plays on until `start` gives another.
  constructor(
    show: (frame: VideoFrame) => void,
    private readonly onError: (message: string) => void,
    private clock: ReviewClock
  ) {
    this.video = new VideoPlayout(show)
  }

  /// The caller anchors `clock` before the call.
  start(clock: ReviewClock, shownFromMicros: number): void {
    this.stop()
    this.clock = clock
    clock.follow(this)
    this.shownFromMicros = shownFromMicros
    if (clock.paused) {
      this.video.setPaused(true)
      this.video.showNextWhilePaused()
    }
  }

  stop(): void {
    this.video.flush()
    this.video.setPaused(false)
    this.audio.flush()
    this.audioDecoder?.close()
    this.audioDecoder = undefined
    this.pendingCaptureMicros.length = 0
    this.clock.unfollow(this)
  }

  followPause(paused: boolean, pausedForMs: number): void {
    if (!paused) {
      this.video.shift(pausedForMs)
    }
    this.video.setPaused(paused)
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
    if (isPcmAlawCodec(config.codec)) {
      for (const chunk of chunks) {
        const audioData = alawToAudioData(
          chunk.data,
          config.sampleRate,
          config.numberOfChannels,
          chunk.captureMicros ?? 0
        )
        if (audioData) {
          this.playDecoded(audioData, chunk.captureMicros)
        }
      }
      return
    }
    if (this.audioDecoder?.state === 'closed') {
      this.audioDecoder = undefined
    }
    this.audioDecoder ??= this.createAudioDecoder(config)
    for (const chunk of chunks) {
      if (chunk.captureMicros === undefined) {
        continue
      }
      this.audioDecoder.decode(new EncodedAudioChunk({ type: 'key', timestamp: chunk.captureMicros, data: chunk.data }))
      this.pendingCaptureMicros.push(chunk.captureMicros)
    }
  }

  setVolume(volume: number): void {
    this.audio.setVolume(volume)
  }

  /// Resolves once the sample captured at `captureMicros` is due, pausing
  /// with playback; `isCurrent` lets a superseded review stop waiting.
  async waitUntilDue(captureMicros: number, isCurrent: () => boolean): Promise<void> {
    while (isCurrent()) {
      const remainingMs = this.dueAt(captureMicros) - performance.now()
      if (remainingMs <= 0 && !this.clock.paused) {
        return
      }
      await new Promise((resolve) => setTimeout(resolve, Math.min(Math.max(remainingMs, 20), WAIT_POLL_MS)))
    }
  }

  /// How far the picture on screen is ahead of the sound.
  syncOffsetMs(): number | undefined {
    return syncOffsetMs(this.video, this.audio)
  }

  private dueAt(captureMicros: number): number {
    return this.clock.dueAt(captureMicros)
  }

  /// Sound that is due already starts at once and moves the clock by how late
  /// it is. When another review's sound moves the clock, as when this review
  /// joins a playing `PlaybackGroup`, late sound is dropped instead: it would
  /// be heard late and over what follows it.
  private playDecoded(audioData: AudioData, captureMicros: number | undefined): void {
    if (
      captureMicros === undefined ||
      captureMicros + (audioData.duration ?? 0) <= this.shownFromMicros ||
      (!this.clock.movedBy(this) && this.dueAt(captureMicros) < performance.now() + this.audio.scheduleAheadMs())
    ) {
      audioData.close()
      return
    }
    this.audio.play(audioData, captureMicros, () => this.dueAt(captureMicros))
    audioData.close()
  }

  private createAudioDecoder(config: AudioDecoderConfig): AudioDecoder {
    const decoder = new AudioDecoder({
      output: (audioData) => this.playDecoded(audioData, this.pendingCaptureMicros.shift()),
      error: (error) => this.onError(`review audio decoder: ${error.message}`)
    })
    decoder.configure(config)
    return decoder
  }
}
