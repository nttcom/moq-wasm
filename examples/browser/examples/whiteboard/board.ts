/// The whiteboard's shared state and the operations that change it. Every operation merges by adding
/// to a set or taking a maximum, so peers that receive the same operations in any order, any number of
/// times, hold the same board. A snapshot is the board written as one operation, which is how a late
/// joiner catches up (see README.md).

/// A Lamport timestamp: `clock` orders operations causally, `author` breaks ties between concurrent ones.
export type Stamp = { clock: number; author: string }

/// `points` is flat `x, y` pairs in [0, 1] of the board. A stroke's ID is its creation stamp.
export type Stroke = { id: string; color: string; width: number; points: number[] }

export type BoardOp =
  | { kind: 'snapshot'; strokes: Stroke[]; erased: string[]; clearedAt?: Stamp }
  | { kind: 'stroke'; stroke: Stroke }
  /// `from` is the offset of these points in the stroke's flat `points`.
  | { kind: 'points'; id: string; from: number; points: number[] }
  | { kind: 'erase'; ids: string[] }
  | { kind: 'clear'; at: Stamp }

export function strokeId(stamp: Stamp): string {
  return `${stamp.clock}@${stamp.author}`
}

export function stampOf(id: string): Stamp | undefined {
  const at = id.indexOf('@')
  const clock = Number(id.slice(0, at))
  return at > 0 && Number.isSafeInteger(clock) ? { clock, author: id.slice(at + 1) } : undefined
}

export function isAfter(a: Stamp, b: Stamp): boolean {
  return a.clock !== b.clock ? a.clock > b.clock : a.author > b.author
}

export class Board {
  private readonly strokes = new Map<string, Stroke>()
  /// IDs of erased strokes, kept so a stroke that arrives after its erase stays hidden.
  private readonly erased = new Set<string>()
  private clearedAt: Stamp | undefined
  /// The largest clock seen, so a local operation is stamped after everything this peer has seen.
  private clock = 0

  nextStamp(author: string): Stamp {
    this.clock += 1
    return { clock: this.clock, author }
  }

  /// Strokes to draw, oldest first.
  visibleStrokes(): Stroke[] {
    return [...this.strokes.values()].sort((a, b) => (isAfter(stampOf(a.id)!, stampOf(b.id)!) ? 1 : -1))
  }

  /// Returns whether the board changed.
  apply(op: BoardOp): boolean {
    switch (op.kind) {
      case 'snapshot': {
        let changed = op.clearedAt ? this.clear(op.clearedAt) : false
        changed = this.erase(op.erased) || changed
        for (const stroke of op.strokes) {
          changed = this.addStroke(stroke) || changed
        }
        return changed
      }
      case 'stroke':
        return this.addStroke(op.stroke)
      case 'points':
        return this.addPoints(op.id, op.from, op.points)
      case 'erase':
        return this.erase(op.ids)
      case 'clear':
        return this.clear(op.at)
    }
  }

  /// Equal boards give equal snapshots.
  snapshot(): BoardOp {
    return {
      kind: 'snapshot',
      strokes: this.visibleStrokes(),
      erased: [...this.erased].sort(),
      ...(this.clearedAt ? { clearedAt: this.clearedAt } : {})
    }
  }

  private addStroke(stroke: Stroke): boolean {
    const stamp = stampOf(stroke.id)
    if (!stamp) {
      return false
    }
    this.observe(stamp)
    if (this.erased.has(stroke.id) || !this.survivesClear(stamp)) {
      return false
    }
    const known = this.strokes.get(stroke.id)
    // Copies of one stroke differ only in how many of its points they have seen.
    if (known && known.points.length >= stroke.points.length) {
      return false
    }
    this.strokes.set(stroke.id, { ...stroke, points: [...stroke.points] })
    return true
  }

  /// Points that do not continue the known ones are dropped; a later snapshot carries them.
  private addPoints(id: string, from: number, points: number[]): boolean {
    const stroke = this.strokes.get(id)
    if (!stroke || from > stroke.points.length || from + points.length <= stroke.points.length) {
      return false
    }
    stroke.points = [...stroke.points.slice(0, from), ...points]
    return true
  }

  private erase(ids: readonly string[]): boolean {
    let changed = false
    for (const id of ids) {
      const stamp = stampOf(id)
      if (!stamp || this.erased.has(id) || !this.survivesClear(stamp)) {
        continue
      }
      this.erased.add(id)
      changed = this.strokes.delete(id) || changed
    }
    return changed
  }

  /// Drops the strokes and erase marks from before `at`, which no later operation can bring back.
  private clear(at: Stamp): boolean {
    this.observe(at)
    if (this.clearedAt && !isAfter(at, this.clearedAt)) {
      return false
    }
    this.clearedAt = at
    let changed = false
    for (const id of this.strokes.keys()) {
      if (!this.survivesClear(stampOf(id)!)) {
        this.strokes.delete(id)
        changed = true
      }
    }
    for (const id of this.erased) {
      if (!this.survivesClear(stampOf(id)!)) {
        this.erased.delete(id)
      }
    }
    return changed
  }

  private survivesClear(stamp: Stamp): boolean {
    return !this.clearedAt || isAfter(stamp, this.clearedAt)
  }

  private observe(stamp: Stamp): void {
    this.clock = Math.max(this.clock, stamp.clock)
  }
}

const MAX_POINTS_PER_STROKE = 20_000

export function encodeOp(op: BoardOp): Uint8Array {
  return new TextEncoder().encode(JSON.stringify(op))
}

/// Peers are untrusted, so a payload that is not a well-formed operation is `undefined`.
export function decodeOp(payload: Uint8Array): BoardOp | undefined {
  let value: unknown
  try {
    value = JSON.parse(new TextDecoder().decode(payload))
  } catch {
    return undefined
  }
  if (!isRecord(value)) {
    return undefined
  }
  switch (value.kind) {
    case 'snapshot':
      return Array.isArray(value.strokes) &&
        value.strokes.every(isStroke) &&
        isIdList(value.erased) &&
        (value.clearedAt === undefined || isStamp(value.clearedAt))
        ? (value as BoardOp)
        : undefined
    case 'stroke':
      return isStroke(value.stroke) ? (value as BoardOp) : undefined
    case 'points':
      return typeof value.id === 'string' && Number.isSafeInteger(value.from) && isPointList(value.points)
        ? (value as BoardOp)
        : undefined
    case 'erase':
      return isIdList(value.ids) ? (value as BoardOp) : undefined
    case 'clear':
      return isStamp(value.at) ? (value as BoardOp) : undefined
    default:
      return undefined
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function isStamp(value: unknown): value is Stamp {
  return isRecord(value) && Number.isSafeInteger(value.clock) && typeof value.author === 'string'
}

function isStroke(value: unknown): value is Stroke {
  return (
    isRecord(value) &&
    typeof value.id === 'string' &&
    typeof value.color === 'string' &&
    typeof value.width === 'number' &&
    isPointList(value.points) &&
    value.points.length >= 2
  )
}

function isPointList(value: unknown): value is number[] {
  return (
    Array.isArray(value) &&
    value.length % 2 === 0 &&
    value.length <= MAX_POINTS_PER_STROKE * 2 &&
    value.every((coordinate) => typeof coordinate === 'number' && coordinate >= 0 && coordinate <= 1)
  )
}

function isIdList(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((id) => typeof id === 'string')
}
