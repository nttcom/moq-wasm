import type { MediaCatalogTrack } from '../msf/catalog'

/// A rendition of the switching set and the bitrate it needs, in bits per second.
export type Rung = { name: string; bitrate: number; height?: number }

/// One object of the rendition being played, when it arrived and how far
/// behind its capture.
export type DeliverySample = { arrivedAtMs: number; bytes: number; lagMs: number | undefined }

const THROUGHPUT_WINDOW_MS = 2_000
/// Samples right after a switch mix the two renditions, whose capture-to-arrival
/// lag differs by the transcoding delay, so they are left out.
const SETTLE_MS = 3_000
/// How far the least lag of the last `RECENT_LAG_WINDOW_MS` may exceed the
/// least lag since the rendition started before it counts as a queue building
/// up on the path. Taking the least lag of a window ignores single large
/// objects such as keyframes.
const CONGESTED_EXCESS_LAG_MS = 400
const RECENT_LAG_WINDOW_MS = 1_000
/// The share of the measured throughput a lower rendition may use.
const THROUGHPUT_SAFETY = 0.8
const INITIAL_UP_HOLD_MS = 10_000
const MAX_UP_HOLD_MS = 120_000
/// Each spell this long without congestion halves a doubled calm spell, down to
/// `INITIAL_UP_HOLD_MS`.
const UP_HOLD_DECAY_MS = 60_000
/// A step down this soon after a step up means the higher rendition did not fit.
const FAILED_UP_MS = 20_000
const RAISE_BLOCK_MS = 10_000

/// The renditions of `current`'s `altGroup`, lowest bitrate first. A track
/// without a catalog bitrate, such as the untranscoded source, is estimated
/// from its pixel count and the largest rendition whose bitrate is known.
/// Without any known bitrate the ladder is empty.
export function bitrateLadder(tracks: MediaCatalogTrack[], current: MediaCatalogTrack): Rung[] {
  if (current.altGroup === undefined) {
    return []
  }
  const set = tracks.filter((track) => track.altGroup === current.altGroup)
  const reference = set
    .filter((track) => track.bitrate !== undefined && pixels(track) !== undefined)
    .sort((a, b) => (pixels(b) ?? 0) - (pixels(a) ?? 0))[0]
  const rungs: Rung[] = []
  for (const track of set) {
    const bitrate = track.bitrate ?? estimateBitrate(track, reference)
    if (bitrate !== undefined) {
      rungs.push({ name: track.name, bitrate, height: track.height })
    }
  }
  return rungs.length > 1 ? rungs.sort((a, b) => a.bitrate - b.bitrate) : []
}

/// The rungs up to the first one at least `height` pixels tall: a picture shown
/// that small gains nothing from a taller rendition.
export function capLadderToHeight(ladder: Rung[], height: number | undefined): Rung[] {
  if (height === undefined) {
    return ladder
  }
  const fitting = ladder.findIndex((rung) => rung.height !== undefined && rung.height >= height)
  return fitting < 0 ? ladder : ladder.slice(0, fitting + 1)
}

function pixels(track: MediaCatalogTrack): number | undefined {
  return track.width && track.height ? track.width * track.height : undefined
}

function estimateBitrate(track: MediaCatalogTrack, reference: MediaCatalogTrack | undefined): number | undefined {
  const trackPixels = pixels(track)
  const referencePixels = reference && pixels(reference)
  if (!reference?.bitrate || !trackPixels || !referencePixels) {
    return undefined
  }
  return (reference.bitrate * trackPixels) / referencePixels
}

/// Chooses the rendition to play from how its objects arrive.
/// - Delivery is pushed at the encoded rate, so throughput measured while the
///   path keeps up does not show how much more it could carry.
/// - A rising capture-to-arrival lag or a stall shows the rendition does not
///   fit; the throughput measured then is what the path carries. With a
///   `targetLagMs`, the least lag of the last second standing above it also
///   counts as not fitting.
/// - A step down goes to the highest rendition that fits that throughput.
/// - A step up tries the next rendition after a calm spell; a failed trial
///   doubles the spell before the next one.
export class AdaptiveBitrate {
  private samples: DeliverySample[] = []
  private settledAtMs = 0
  private baselineLagMs: number | undefined
  private calmSinceMs = 0
  private upHoldMs = INITIAL_UP_HOLD_MS
  private upHoldChangedAtMs = 0
  private lastUpAtMs: number | undefined
  private lastCongestedAtMs: number | undefined

  constructor(private readonly targetLagMs?: number) {}

  /// A rendition started playing.
  restart(nowMs: number): void {
    this.samples = []
    this.settledAtMs = nowMs + SETTLE_MS
    this.baselineLagMs = undefined
    this.calmSinceMs = this.settledAtMs
  }

  observe(sample: DeliverySample): void {
    if (sample.arrivedAtMs < this.settledAtMs) {
      return
    }
    this.samples.push(sample)
    const horizon = sample.arrivedAtMs - Math.max(THROUGHPUT_WINDOW_MS, RECENT_LAG_WINDOW_MS)
    while (this.samples.length > 0 && this.samples[0].arrivedAtMs < horizon) {
      this.samples.shift()
    }
    if (sample.lagMs !== undefined && (this.baselineLagMs === undefined || sample.lagMs < this.baselineLagMs)) {
      this.baselineLagMs = sample.lagMs
    }
  }

  /// Returns the rendition to switch to, or undefined to stay on `current`.
  decide(nowMs: number, ladder: Rung[], current: string, stalled: boolean): string | undefined {
    const index = ladder.findIndex((rung) => rung.name === current)
    if (index < 0 || nowMs < this.settledAtMs) {
      return undefined
    }
    if (stalled || this.congested(nowMs)) {
      this.calmSinceMs = nowMs
      this.lastCongestedAtMs = nowMs
      return this.stepDown(nowMs, ladder, index)
    }
    this.relaxUpHold(nowMs)
    if (index + 1 < ladder.length && nowMs - this.calmSinceMs >= this.upHoldMs) {
      this.lastUpAtMs = nowMs
      return ladder[index + 1].name
    }
    return undefined
  }

  /// Returns the top of `ladder` when it lies above `current`, after something other than
  /// the path raised the ladder's ceiling, such as a picture shown taller. Counts as a step up.
  /// Returns undefined while the path was congested in the last `RAISE_BLOCK_MS`.
  raiseTo(nowMs: number, ladder: Rung[], current: string): string | undefined {
    const index = ladder.findIndex((rung) => rung.name === current)
    const top = ladder.at(-1)
    const congestedRecently = this.lastCongestedAtMs !== undefined && nowMs - this.lastCongestedAtMs < RAISE_BLOCK_MS
    if (!top || index < 0 || index === ladder.length - 1 || congestedRecently) {
      return undefined
    }
    this.lastUpAtMs = nowMs
    return top.name
  }

  private relaxUpHold(nowMs: number): void {
    const calmFromMs = Math.max(this.lastCongestedAtMs ?? 0, this.upHoldChangedAtMs)
    if (this.upHoldMs > INITIAL_UP_HOLD_MS && nowMs - calmFromMs >= UP_HOLD_DECAY_MS) {
      this.upHoldMs = Math.max(INITIAL_UP_HOLD_MS, this.upHoldMs / 2)
      this.upHoldChangedAtMs = nowMs
    }
  }

  private congested(nowMs: number): boolean {
    if (this.baselineLagMs === undefined) {
      return false
    }
    let recentLeastLagMs: number | undefined
    for (const sample of this.samples) {
      if (sample.arrivedAtMs >= nowMs - RECENT_LAG_WINDOW_MS && sample.lagMs !== undefined) {
        recentLeastLagMs = Math.min(recentLeastLagMs ?? sample.lagMs, sample.lagMs)
      }
    }
    return (
      recentLeastLagMs !== undefined &&
      (recentLeastLagMs - this.baselineLagMs > CONGESTED_EXCESS_LAG_MS ||
        (this.targetLagMs !== undefined && recentLeastLagMs > this.targetLagMs))
    )
  }

  private stepDown(nowMs: number, ladder: Rung[], index: number): string | undefined {
    if (index === 0) {
      return undefined
    }
    if (this.lastUpAtMs !== undefined && nowMs - this.lastUpAtMs < FAILED_UP_MS) {
      this.upHoldMs = Math.min(this.upHoldMs * 2, MAX_UP_HOLD_MS)
      this.upHoldChangedAtMs = nowMs
    }
    this.lastUpAtMs = undefined
    const fits = THROUGHPUT_SAFETY * this.throughputBps(nowMs)
    let target = 0
    for (let candidate = index - 1; candidate > 0; candidate -= 1) {
      if (ladder[candidate].bitrate <= fits) {
        target = candidate
        break
      }
    }
    return ladder[target].name
  }

  private throughputBps(nowMs: number): number {
    let bytes = 0
    for (const sample of this.samples) {
      if (sample.arrivedAtMs >= nowMs - THROUGHPUT_WINDOW_MS) {
        bytes += sample.bytes
      }
    }
    return (bytes * 8 * 1_000) / THROUGHPUT_WINDOW_MS
  }
}
