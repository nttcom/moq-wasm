import { AudioPlayout } from './audioPlayout'
import { type BufferPolicy, JitterBuffer } from './jitterBuffer'
import { PlayoutClock, type PlayoutOrigin } from './playoutClock'
import { VideoPlayout } from './videoPlayout'

export const DEFAULT_BUFFER_POLICY: BufferPolicy = { minimumMs: 200, maximumMs: Number.POSITIVE_INFINITY }
export type CatchUp = 'speed-up' | 'skip' | 'off'
const WARMUP_MS = 400
const CATCH_UP_COMPRESSION_RATE = 1.1
const CATCH_UP_EXCESS_MS = 20
const MAX_CATCH_UP_MS = 400
const MICROS_PER_MILLI = 1_000

type Held = { captureMicros: number; arrivedAtMs: number } & (
  | { kind: 'video'; frame: VideoFrame }
  | { kind: 'audio'; audioData: AudioData }
)

/// Live LOC playback. Both decoders hand over samples as soon as they are
/// decoded and one clock decides when each is presented, which is what keeps
/// the picture and the sound together. The audio is the clock's master: it
/// alone moves the clock, by the drift the audio device shows and by the
/// misses it takes, so the sound never has to skip for the picture; the
/// picture takes over only while no audio is playing. Every move of the clock
/// applies from the sample that caused it on, so the frames waiting for their
/// time move with the sound they belong to. A sample without a capture
/// timestamp is presented at once.
///
/// Playback opens with a warm-up: a subscription starts with a burst of what
/// the relay had cached of the current groups, so the samples of the first
/// `WARMUP_MS` are held and the clock is anchored on the newest of them. The
/// burst is not observed; the buffer opens on the longest wait between two
/// audio arrivals during the warm-up instead, because sources
/// such as MPEG-TS over SRT deliver audio in bursts of a few hundred
/// milliseconds. A buffer below its target grows by the misses it takes. Once
/// the buffer has settled, one above its target is shed as the catch-up
/// asks: `speed-up` compresses the audio while it is more than
/// `CATCH_UP_EXCESS_MS` over, `skip` drops the audio chunks that fit in the
/// excess, `speed-up` skips too once it is more than `MAX_CATCH_UP_MS` over,
/// and `off` keeps it. Whatever is older than the buffer is dropped rather
/// than played late. Pausing drops what arrives, and resuming warms up again
/// at the live edge.
export class LivePlayout {
  private readonly clock = new PlayoutClock()
  private readonly jitterBuffer = new JitterBuffer(DEFAULT_BUFFER_POLICY)
  private readonly audio = new AudioPlayout((driftMs, captureMicros) => this.shiftClock(driftMs, captureMicros))
  private readonly video: VideoPlayout
  private warmup: { timer: ReturnType<typeof setTimeout>; held: Held[] } | undefined
  private newestAudioCaptureMicros: number | undefined
  private paused = false
  private catchUp: CatchUp = 'skip'
  private videoShedMs = 0

  constructor(showVideo: (frame: VideoFrame) => void, onTimeline?: (origin: PlayoutOrigin | undefined) => void) {
    this.video = new VideoPlayout(showVideo)
    if (onTimeline) {
      this.clock.setOriginListener(onTimeline)
    }
  }

  presentVideo(frame: VideoFrame): void {
    if (this.paused) {
      frame.close()
      return
    }
    const captureMicros = frame.timestamp
    if (!captureMicros) {
      this.video.present(frame, performance.now())
      return
    }
    if (!this.clock.anchored) {
      this.hold({ kind: 'video', frame, captureMicros, arrivedAtMs: performance.now() })
      return
    }
    const nowMs = performance.now()
    const master = !this.audioActive()
    if (master) {
      this.jitterBuffer.observe(captureMicros, nowMs)
      const excessMs = this.catchUp === 'off' ? undefined : this.excessMs()
      if (excessMs !== undefined && excessMs > MAX_CATCH_UP_MS) {
        this.shiftClock(-excessMs, captureMicros)
        this.videoShedMs += excessMs
      }
    }
    this.video.present(frame, this.playoutTime(captureMicros, nowMs, master))
  }

  playAudio(audioData: AudioData, captureMicros: number | undefined): void {
    if (this.paused) {
      audioData.close()
      return
    }
    this.audio.prepare(audioData.sampleRate)
    if (!captureMicros) {
      this.audio.play(audioData, undefined, (nowMs) => nowMs)
      audioData.close()
      return
    }
    if (!this.clock.anchored) {
      this.hold({ kind: 'audio', audioData, captureMicros, arrivedAtMs: performance.now() })
      return
    }
    const arrivedAtMs = performance.now()
    if (this.scheduleAudio(audioData, captureMicros)) {
      this.jitterBuffer.observe(captureMicros, arrivedAtMs)
    }
  }

  setVolume(volume: number): void {
    this.audio.setVolume(volume)
  }

  /// Warms up again so a larger buffer applies to what arrives from now on;
  /// what was already scheduled plays out on the old timeline.
  setBufferPolicy(policy: BufferPolicy): void {
    this.jitterBuffer.setPolicy(policy)
    this.clock.reset()
    this.newestAudioCaptureMicros = undefined
  }

  bufferPolicy(): BufferPolicy {
    return this.jitterBuffer.currentPolicy
  }

  fixedBuffer(): boolean {
    return this.jitterBuffer.fixed
  }

  setCatchUp(catchUp: CatchUp): void {
    this.catchUp = catchUp
  }

  /// The sound has to reach the audio device before it is due, so the
  /// buffer covers the output latency on top of the arrival spread.
  targetBufferMs(): number {
    return this.jitterBuffer.targetMs() + (this.audioActive() ? this.audio.outputLatencyMs() : 0)
  }

  /// How long the fastest arrival of the window waits before it is presented.
  bufferMs(): number | undefined {
    const offsetMs = this.clock.offsetMs
    const fastestDelayMs = this.jitterBuffer.fastestDelayMs()
    return offsetMs === undefined || fastestDelayMs === undefined ? undefined : offsetMs - fastestDelayMs
  }

  setPaused(paused: boolean): void {
    if (this.paused === paused) {
      return
    }
    this.paused = paused
    if (paused) {
      this.video.flush()
      this.dropHeld()
      this.audio.flush()
      void this.audio.suspend()
    } else {
      this.clock.reset()
      this.newestAudioCaptureMicros = undefined
      void this.audio.resume()
    }
  }

  reset(): void {
    this.video.flush()
    this.audio.flush()
    this.dropHeld()
    this.clock.reset()
    this.jitterBuffer.reset()
    this.newestAudioCaptureMicros = undefined
  }

  /// How many times the sound has not continued where the previous chunk
  /// ended since playback started.
  audioBreaks(): number {
    return this.audio.breaks
  }

  videoDrops(): string {
    return `${this.video.dropped} dropped / ${this.video.late} late`
  }

  /// How much latency the catch-up has taken out since playback started.
  shedMs(): number {
    return this.audio.shedMs + this.videoShedMs
  }

  /// How far the picture on screen is ahead of the sound, from the capture
  /// timestamps each was presented at.
  syncOffsetMs(): number | undefined {
    const nowMs = performance.now()
    const video = this.video.positionMicrosAt(nowMs)
    const audio = this.audio.positionMicrosAt(nowMs)
    if (video === undefined || audio === undefined) {
      return undefined
    }
    return (video - audio) / MICROS_PER_MILLI
  }

  /// The relay sends the groups of a fresh subscription on separate streams,
  /// so a chunk of an older group can land after a newer one has been
  /// scheduled. The write head has moved past it, and as master it would
  /// drag the clock back, so it is dropped.
  private scheduleAudio(audioData: AudioData, captureMicros: number): boolean {
    if (this.newestAudioCaptureMicros !== undefined && captureMicros <= this.newestAudioCaptureMicros) {
      audioData.close()
      return false
    }
    this.newestAudioCaptureMicros = captureMicros
    const excessMs = this.catchUp === 'off' ? 0 : (this.excessMs() ?? 0)
    const durationMs = audioData.duration / MICROS_PER_MILLI
    if (excessMs > durationMs && (this.catchUp === 'skip' || excessMs > MAX_CATCH_UP_MS)) {
      this.audio.skip(audioData)
      this.shiftClock(-durationMs, captureMicros)
      audioData.close()
      return true
    }
    this.audio.setCompressionRate(
      this.catchUp === 'speed-up' && excessMs > CATCH_UP_EXCESS_MS ? CATCH_UP_COMPRESSION_RATE : 1
    )
    this.audio.play(audioData, captureMicros, (nowMs) => this.playoutTime(captureMicros, nowMs, true))
    audioData.close()
    return true
  }

  private excessMs(): number | undefined {
    const bufferMs = this.bufferMs()
    return bufferMs === undefined || !this.jitterBuffer.settled ? undefined : bufferMs - this.targetBufferMs()
  }

  private audioActive(): boolean {
    return this.audio.positionMicrosAt(performance.now()) !== undefined
  }

  /// A master sample that misses its time is presented at once and pushes
  /// the clock back by the miss so the samples behind it stay contiguous.
  private playoutTime(captureMicros: number, nowMs: number, master: boolean): number {
    const due = this.clock.dueAt(captureMicros) ?? nowMs
    if (due >= nowMs) {
      return due
    }
    if (master) {
      this.shiftClock(nowMs - due, captureMicros)
    }
    return nowMs
  }

  private shiftClock(deltaMs: number, fromCaptureMicros: number | undefined): void {
    this.clock.shift(deltaMs)
    this.video.shift(deltaMs, fromCaptureMicros)
  }

  private hold(sample: Held): void {
    this.warmup ??= { timer: setTimeout(() => this.endWarmup(), WARMUP_MS), held: [] }
    this.warmup.held.push(sample)
  }

  private endWarmup(): void {
    if (!this.warmup) {
      return
    }
    const { held } = this.warmup
    this.warmup = undefined
    if (held.length === 0) {
      return
    }
    const nowMs = performance.now()
    const newest = Math.max(...held.map((sample) => sample.captureMicros))
    this.clock.anchor(newest, nowMs + this.jitterBuffer.openingTargetMs(longestArrivalGapMs(held)))
    for (const sample of held.sort((left, right) => left.captureMicros - right.captureMicros)) {
      const due = this.clock.dueAt(sample.captureMicros) ?? nowMs
      if (due < nowMs) {
        close(sample)
      } else if (sample.kind === 'video') {
        this.video.present(sample.frame, due)
      } else {
        this.scheduleAudio(sample.audioData, sample.captureMicros)
      }
    }
  }

  private dropHeld(): void {
    if (!this.warmup) {
      return
    }
    clearTimeout(this.warmup.timer)
    for (const sample of this.warmup.held) {
      close(sample)
    }
    this.warmup = undefined
  }
}

function longestArrivalGapMs(held: Held[]): number {
  const audio = held.filter((sample) => sample.kind === 'audio')
  const arrivals = (audio.length > 1 ? audio : held).map((sample) => sample.arrivedAtMs).sort((a, b) => a - b)
  let longest = 0
  for (let i = 1; i < arrivals.length; i += 1) {
    longest = Math.max(longest, arrivals[i] - arrivals[i - 1])
  }
  return longest
}

function close(sample: Held): void {
  if (sample.kind === 'video') {
    sample.frame.close()
  } else {
    sample.audioData.close()
  }
}
