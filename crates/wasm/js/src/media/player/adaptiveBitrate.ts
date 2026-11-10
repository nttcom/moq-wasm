import type { MediaCatalogTrack } from '../catalog'

/// A rendition of the switching set and the bitrate it needs, in bits per second.
export type Rung = { name: string; bitrate: number; height?: number }

/// One object of the rendition being played, when it arrived and how far
/// behind its capture.
export type DeliverySample = { arrivedAtMs: number; bytes: number; lagMs: number | undefined }

/// Why `decideUnderHeight` switches: the picture is shorter than the rendition
/// (`height`), a taller picture raised the cap (`raise`), the path is congested
/// (`congested`), or a calm spell tries the next rendition (`calm`).
type SwitchReason = 'height' | 'raise' | 'congested' | 'calm'

/// A switch with what the delivery measured when it was decided: the least lag
/// of the last `RECENT_LAG_WINDOW_MS`, the least lag since the rendition
/// started, and the throughput of the last `THROUGHPUT_WINDOW_MS`.
export type RenditionChoice = {
  name: string
  reason: SwitchReason
  recentLagMs: number | undefined
  baselineLagMs: number | undefined
  throughputBps: number
}

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
/// Long enough to hold a keyframe at the meeting camera's 2 s interval.
const SPREAD_WINDOW_MS = 3_000
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

/// The top of the ladder capped to `height` when `current` lies above it, or
/// undefined when `current` fits or is not on the ladder.
export function renditionAboveHeight(ladder: Rung[], current: string, height: number | undefined): string | undefined {
  const capped = capLadderToHeight(ladder, height)
  if (!ladder.some((rung) => rung.name === current) || capped.some((rung) => rung.name === current)) {
    return undefined
  }
  return capped.at(-1)?.name
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
/// - A rising capture-to-arrival lag shows the rendition does not fit; the
///   throughput measured then is what the path carries. A gap in arrivals
///   does not count: a publisher sends nothing while its source does not
///   change, such as a shared screen, and objects a congested path holds up
///   show the queue in their lag once they arrive. With a
///   `targetLagMs`, the largest lag of the last `SPREAD_WINDOW_MS` standing
///   above it also counts as not fitting, since the jitter buffer holds the
///   playout back by the spread keyframes add to the least lag. The target is
///   read at every decision.
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
  private ceilingBitrate: number | undefined

  constructor(private readonly targetLagMs?: () => number) {}

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
    const horizon = sample.arrivedAtMs - Math.max(THROUGHPUT_WINDOW_MS, RECENT_LAG_WINDOW_MS, SPREAD_WINDOW_MS)
    while (this.samples.length > 0 && this.samples[0].arrivedAtMs < horizon) {
      this.samples.shift()
    }
    if (sample.lagMs !== undefined && (this.baselineLagMs === undefined || sample.lagMs < this.baselineLagMs)) {
      this.baselineLagMs = sample.lagMs
    }
  }

  /// Returns the rendition to switch to, or undefined to stay on `current`.
  decide(nowMs: number, ladder: Rung[], current: string): string | undefined {
    const index = ladder.findIndex((rung) => rung.name === current)
    if (index < 0 || nowMs < this.settledAtMs) {
      return undefined
    }
    if (this.congested(nowMs)) {
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

  /// `decide` over the ladder capped to the picture's `height`: a rendition
  /// above the cap steps down to it at once, and a cap raised by a taller
  /// picture is taken as `raiseTo` allows.
  decideUnderHeight(
    nowMs: number,
    ladder: Rung[],
    current: string,
    height: number | undefined
  ): RenditionChoice | undefined {
    const capped = capLadderToHeight(ladder, height)
    const ceilingBitrate = capped.at(-1)?.bitrate
    const raised =
      this.ceilingBitrate !== undefined && ceilingBitrate !== undefined && ceilingBitrate > this.ceilingBitrate
    this.ceilingBitrate = ceilingBitrate
    const choose = (name: string, reason: SwitchReason): RenditionChoice => ({
      name,
      reason,
      ...this.measure(nowMs)
    })
    const above = renditionAboveHeight(ladder, current, height)
    if (above) {
      return choose(above, 'height')
    }
    const raisedTo = raised ? this.raiseTo(nowMs, capped, current) : undefined
    if (raisedTo) {
      return choose(raisedTo, 'raise')
    }
    const decided = this.decide(nowMs, capped, current)
    return decided ? choose(decided, this.lastCongestedAtMs === nowMs ? 'congested' : 'calm') : undefined
  }

  private measure(nowMs: number): Omit<RenditionChoice, 'name' | 'reason'> {
    const recentLags = this.lagsSince(nowMs - RECENT_LAG_WINDOW_MS)
    return {
      recentLagMs: recentLags.length > 0 ? Math.min(...recentLags) : undefined,
      baselineLagMs: this.baselineLagMs,
      throughputBps: this.throughputBps(nowMs)
    }
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
    const recentLags = this.lagsSince(nowMs - RECENT_LAG_WINDOW_MS)
    if (recentLags.length > 0 && Math.min(...recentLags) - this.baselineLagMs > CONGESTED_EXCESS_LAG_MS) {
      return true
    }
    const targetLagMs = this.targetLagMs?.()
    const spreadLags = this.lagsSince(nowMs - SPREAD_WINDOW_MS)
    return targetLagMs !== undefined && spreadLags.length > 0 && Math.max(...spreadLags) > targetLagMs
  }

  private lagsSince(fromMs: number): number[] {
    return this.samples.flatMap((sample) =>
      sample.arrivedAtMs >= fromMs && sample.lagMs !== undefined ? [sample.lagMs] : []
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
