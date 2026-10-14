import type { MediaCatalogTrack } from '../msf/catalog'

/// A rendition of the switching set and the bitrate it needs, in bits per second.
export type Rung = { name: string; bitrate: number }

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
/// A step down this soon after a step up means the higher rendition did not fit.
const FAILED_UP_MS = 20_000

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
      rungs.push({ name: track.name, bitrate })
    }
  }
  return rungs.length > 1 ? rungs.sort((a, b) => a.bitrate - b.bitrate) : []
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
///   fit; the throughput measured then is what the path carries.
/// - A step down goes to the highest rendition that fits that throughput.
/// - A step up tries the next rendition after a calm spell; a failed trial
///   doubles the spell before the next one.
export class AdaptiveBitrate {
  private samples: DeliverySample[] = []
  private settledAtMs = 0
  private baselineLagMs: number | undefined
  private calmSinceMs = 0
  private upHoldMs = INITIAL_UP_HOLD_MS
  private lastUpAtMs: number | undefined

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
      return this.stepDown(nowMs, ladder, index)
    }
    if (index + 1 < ladder.length && nowMs - this.calmSinceMs >= this.upHoldMs) {
      this.lastUpAtMs = nowMs
      return ladder[index + 1].name
    }
    return undefined
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
    return recentLeastLagMs !== undefined && recentLeastLagMs - this.baselineLagMs > CONGESTED_EXCESS_LAG_MS
  }

  private stepDown(nowMs: number, ladder: Rung[], index: number): string | undefined {
    if (index === 0) {
      return undefined
    }
    if (this.lastUpAtMs !== undefined && nowMs - this.lastUpAtMs < FAILED_UP_MS) {
      this.upHoldMs = Math.min(this.upHoldMs * 2, MAX_UP_HOLD_MS)
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
