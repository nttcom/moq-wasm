import { PlayoutClock } from './playoutClock'

/// The first frame of a review is due this long after the clock is anchored,
/// so it has been fetched and decoded by then.
const REVIEW_LEAD_MS = 150

/// A review playing on the clock: it holds its frames while the clock is
/// paused and moves them by the pause when it resumes.
export interface ClockFollower {
  followPause(paused: boolean, pausedForMs: number): void
}

/// The clock a review is presented on. It maps capture timestamps onto local
/// time from the position it is anchored at, owns the AudioContext the
/// review's sound plays through, and holds the pause: pausing freezes the
/// position and suspends the context, and resuming moves every time the clock
/// gives, and the frames its followers hold, by the pause. The drift of the
/// review's sound moves it, so the picture keeps step with the sound. Every
/// review on it starts again after `anchor`.
export class ReviewClock {
  private readonly clock = new PlayoutClock()
  private readonly followers = new Set<ClockFollower>()
  private context: AudioContext | undefined
  private pausedAtMs: number | undefined

  get paused(): boolean {
    return this.pausedAtMs !== undefined
  }

  anchor(captureMicros: number, paused: boolean): void {
    this.clock.anchor(captureMicros, performance.now() + REVIEW_LEAD_MS)
    this.pausedAtMs = paused ? performance.now() : undefined
    this.applyPauseToContext()
  }

  reset(): void {
    this.clock.reset()
    this.pausedAtMs = undefined
    this.applyPauseToContext()
  }

  /// When the sample captured at `captureMicros` is due; now while the clock
  /// is not anchored.
  dueAt(captureMicros: number): number {
    return this.clock.dueAt(captureMicros) ?? performance.now()
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
  }

  /// `driftMs` is how far the review's sound has run from the clock.
  drift(driftMs: number): void {
    this.clock.shift(driftMs)
  }

  /// The context is created on first use, after the page's gesture, so that
  /// the autoplay policy lets it run, and runs at the stream's sample rate.
  /// It is replaced when the sample rate changes so no chunk is resampled on
  /// its own.
  audioContext(sampleRate: number): AudioContext {
    if (this.context && this.context.sampleRate !== sampleRate) {
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
