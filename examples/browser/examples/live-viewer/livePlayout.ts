import { AudioPlayout } from './audioPlayout'
import { PlayoutClock } from './playoutClock'
import { VideoPlayout } from './videoPlayout'

const PLAYOUT_DELAY_MS = 200
const WARMUP_MS = 400
const MAX_EARLY_MS = 400
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
/// picture takes over only while no audio is playing. A sample without a
/// capture timestamp is presented at once.
///
/// Playback opens with a warm-up: a subscription starts with a burst of what
/// the relay had cached of the current groups, so the samples of the first
/// `WARMUP_MS` are held and the clock is anchored on the newest of them.
/// Sources such as MPEG-TS over SRT deliver audio in bursts of a few hundred
/// milliseconds; the longest wait between two audio arrivals during the
/// warm-up is added to the budget so the head of every later burst is still
/// on time. Whatever is older than the budget is dropped rather than played
/// late. Pausing drops what arrives, and resuming warms up again at the live
/// edge.
export class LivePlayout {
  private readonly clock = new PlayoutClock(PLAYOUT_DELAY_MS, MAX_EARLY_MS)
  private readonly audio = new AudioPlayout((driftMs) => this.clock.shift(driftMs))
  private readonly video: VideoPlayout
  private warmup: { timer: ReturnType<typeof setTimeout>; held: Held[] } | undefined
  private newestAudioCaptureMicros: number | undefined
  private paused = false

  constructor(showVideo: (frame: VideoFrame) => void) {
    this.video = new VideoPlayout(showVideo)
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
    this.video.present(frame, this.playoutTime(captureMicros, performance.now(), !this.audioActive()))
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
    this.scheduleAudio(audioData, captureMicros)
  }

  setVolume(volume: number): void {
    this.audio.setVolume(volume)
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
    this.newestAudioCaptureMicros = undefined
  }

  /// How many times the sound has not continued where the previous chunk
  /// ended since playback started.
  audioBreaks(): number {
    return this.audio.breaks
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
  private scheduleAudio(audioData: AudioData, captureMicros: number): void {
    if (this.newestAudioCaptureMicros !== undefined && captureMicros <= this.newestAudioCaptureMicros) {
      audioData.close()
      return
    }
    this.newestAudioCaptureMicros = captureMicros
    this.audio.play(audioData, captureMicros, (nowMs) => this.playoutTime(captureMicros, nowMs, true))
    audioData.close()
  }

  private audioActive(): boolean {
    return this.audio.positionMicrosAt(performance.now()) !== undefined
  }

  private playoutTime(captureMicros: number, nowMs: number, master: boolean): number {
    const { atMs, reanchored } = this.clock.playoutTime(captureMicros, nowMs, master)
    if (reanchored) {
      this.video.flush()
      this.audio.dropScheduled()
    }
    return atMs
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
    this.clock.anchor(newest, nowMs, PLAYOUT_DELAY_MS + longestArrivalGapMs(held))
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
