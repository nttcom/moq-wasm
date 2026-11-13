/// One cursor sample: `x` and `y` are in [0, 1] of the pad, `sentAtMs` is the
/// sender's `performance.timeOrigin + performance.now()`.
export type CursorSample = { seq: number; x: number; y: number; sentAtMs: number }

const SAMPLE_BYTES = 20

export function encodeCursorSample({ seq, x, y, sentAtMs }: CursorSample): Uint8Array {
  const bytes = new Uint8Array(SAMPLE_BYTES)
  const view = new DataView(bytes.buffer)
  view.setUint32(0, seq)
  view.setFloat32(4, x)
  view.setFloat32(8, y)
  view.setFloat64(12, sentAtMs)
  return bytes
}

export function decodeCursorSample(payload: Uint8Array): CursorSample | undefined {
  if (payload.byteLength !== SAMPLE_BYTES) {
    return undefined
  }
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength)
  return { seq: view.getUint32(0), x: view.getFloat32(4), y: view.getFloat32(8), sentAtMs: view.getFloat64(12) }
}

export type LatencyPoint = { receivedAtMs: number; latencyMs: number }

export type TrackSummary = {
  received: number
  /// Samples between the first and the largest seq that never arrived.
  lost: number
  /// Samples that arrived after one with a larger seq.
  reordered: number
  lastLatencyMs: number | undefined
  medianLatencyMs: number | undefined
  p95LatencyMs: number | undefined
  /// The longest wait between two arrivals within the window.
  maxGapMs: number | undefined
}

/// Counts what one track delivered and keeps the latencies of the last `windowMs`.
export class TrackStats {
  private received = 0
  private reordered = 0
  private firstSeq: number | undefined
  private largestSeq: number | undefined
  private points: LatencyPoint[] = []

  constructor(private readonly windowMs: number) {}

  record(sample: CursorSample, receivedAtMs: number): void {
    this.received += 1
    if (this.firstSeq === undefined || this.largestSeq === undefined) {
      this.firstSeq = sample.seq
      this.largestSeq = sample.seq
    } else if (sample.seq < this.largestSeq) {
      this.reordered += 1
      this.firstSeq = Math.min(this.firstSeq, sample.seq)
    } else {
      this.largestSeq = sample.seq
    }
    this.points.push({ receivedAtMs, latencyMs: receivedAtMs - sample.sentAtMs })
    this.prune(receivedAtMs)
  }

  /// The latencies of the window ending at `nowMs`, oldest first.
  latencies(nowMs: number): readonly LatencyPoint[] {
    this.prune(nowMs)
    return this.points
  }

  summary(nowMs: number): TrackSummary {
    const points = this.latencies(nowMs)
    const sorted = points.map((point) => point.latencyMs).sort((a, b) => a - b)
    let maxGapMs: number | undefined
    for (let index = 1; index < points.length; index += 1) {
      const gap = points[index].receivedAtMs - points[index - 1].receivedAtMs
      maxGapMs = Math.max(maxGapMs ?? 0, gap)
    }
    const expected =
      this.firstSeq === undefined || this.largestSeq === undefined ? 0 : this.largestSeq - this.firstSeq + 1
    return {
      received: this.received,
      lost: Math.max(0, expected - this.received),
      reordered: this.reordered,
      lastLatencyMs: points.at(-1)?.latencyMs,
      medianLatencyMs: percentile(sorted, 0.5),
      p95LatencyMs: percentile(sorted, 0.95),
      maxGapMs
    }
  }

  private prune(nowMs: number): void {
    const start = this.points.findIndex((point) => point.receivedAtMs >= nowMs - this.windowMs)
    this.points = start < 0 ? [] : this.points.slice(start)
  }
}

function percentile(sorted: readonly number[], rank: number): number | undefined {
  if (sorted.length === 0) {
    return undefined
  }
  return sorted[Math.min(sorted.length - 1, Math.floor(rank * sorted.length))]
}
