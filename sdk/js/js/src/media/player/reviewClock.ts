import { PlayoutClock } from './playoutClock'

const MICROS_PER_MILLI = 1_000
/// The first frame of a review is due this long after the clock is anchored,
/// so it has been fetched and decoded by then.
const REVIEW_LEAD_MS = 150

/// A review playing on the clock: it holds its frames while the clock is
/// paused and moves them by the pause when it resumes.
export interface ClockFollower {
  followPause(paused: boolean, pausedForMs: number): void
}

/// The clock a review is presented on. It maps capture timestamps onto local
/// time from the position it is anchored at, and owns the AudioContext the
/// review's sound plays through. Pausing freezes the position and suspends
/// the context; resuming moves every time the clock gives, and the frames its
/// followers hold, by the pause. Every review on it starts again after
/// `anchor`.
///
/// A player owns one; a `PlaybackGroup` shares one between its players, so
/// they present the same capture time at the same moment. The sound's drift
/// moves the clock so the pictures keep step with the sound, but only the
/// drift of the first follower to report it since the anchor, until that
/// follower stops following. Every review plays through the one context, on
/// the same audio device, so the others' sound keeps step with it too.
export class ReviewClock {
  private readonly clock = new PlayoutClock()
  private readonly followers = new Set<ClockFollower>()
  private driftSource: ClockFollower | undefined
  private context: AudioContext | undefined
  private anchorMicros: number | undefined
  private pausedAtMs: number | undefined

  get paused(): boolean {
    return this.pausedAtMs !== undefined
  }

  anchor(captureMicros: number, paused: boolean): void {
    this.clock.anchor(captureMicros, performance.now() + REVIEW_LEAD_MS)
    this.anchorMicros = captureMicros
    this.driftSource = undefined
    this.pausedAtMs = paused ? performance.now() : undefined
    this.applyPauseToContext()
  }

  /// Releases the audio context; a later review creates another.
  close(): void {
    void this.context?.close()
    this.context = undefined
  }

  reset(): void {
    this.clock.reset()
    this.anchorMicros = undefined
    this.driftSource = undefined
    this.pausedAtMs = undefined
    this.applyPauseToContext()
  }

  /// When the sample captured at `captureMicros` is due; now while the clock
  /// is not anchored.
  dueAt(captureMicros: number): number {
    return this.clock.dueAt(captureMicros) ?? performance.now()
  }

  /// The capture time presented now, never before the anchor and frozen while
  /// paused; undefined while the clock is not anchored.
  positionMicros(): number | undefined {
    const offsetMs = this.clock.offsetMs
    if (offsetMs === undefined || this.anchorMicros === undefined) {
      return undefined
    }
    const atMs = this.pausedAtMs ?? performance.now()
    return Math.max(this.anchorMicros, (atMs - offsetMs) * MICROS_PER_MILLI)
  }

  setPaused(paused: boolean): void {
    if (paused === this.paused) {
      return
    }
    if (paused) {
      this.pausedAtMs = performance.now()
      for (const follower of this.followers) {
        follower.followPause(true, 0)
      }
      this.applyPauseToContext()
      return
    }
    const pausedForMs = performance.now() - (this.pausedAtMs ?? 0)
    this.pausedAtMs = undefined
    this.clock.shift(pausedForMs)
    for (const follower of this.followers) {
      follower.followPause(false, pausedForMs)
    }
    this.applyPauseToContext()
  }

  follow(follower: ClockFollower): void {
    this.followers.add(follower)
  }

  unfollow(follower: ClockFollower): void {
    this.followers.delete(follower)
    if (this.driftSource === follower) {
      this.driftSource = undefined
    }
  }

  /// `driftMs` is how far the sound of `follower` has run from the clock.
  drift(follower: ClockFollower, driftMs: number): void {
    this.driftSource ??= follower
    if (this.driftSource === follower) {
      this.clock.shift(driftMs)
    }
  }

  /// Whether the drift `follower` reports would move the clock.
  movedBy(follower: ClockFollower): boolean {
    return this.driftSource === undefined || this.driftSource === follower
  }

  /// The context is created on first use, after the page's gesture, so the
  /// autoplay policy lets it run. It runs at the stream's sample rate and is
  /// replaced when that rate changes, so no chunk is resampled on its own.
  /// While another review plays on it, it is kept instead, and a stream at
  /// another rate is resampled to the context's.
  audioContext(sampleRate: number): AudioContext {
    if (this.context && this.context.sampleRate !== sampleRate && this.followers.size <= 1) {
      void this.context.close()
      this.context = undefined
    }
    if (!this.context) {
      this.context = new AudioContext({ sampleRate })
      this.applyPauseToContext()
    }
    return this.context
  }

  private applyPauseToContext(): void {
    if (!this.context) {
      return
    }
    if (this.paused) {
      void this.context.suspend()
    } else if (this.context.state === 'suspended') {
      void this.context.resume()
    }
  }
}
