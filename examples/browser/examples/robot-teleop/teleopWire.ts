/// The records of a robot room (docs/architecture/robotics/architecture.md): each
/// object of an event timeline track is a JSON array of `{"T": ms, "data": ...}`.

export type RobotState = { t: number; x: number; y: number }
export type Control = { driver: string | null; reason: string }
/// `entries` repeats `velocity`, entry `i` meant for robot time `basis + (i + 1) × dtMs`.
export type ActionChunk = { seq: number; basis: number; dtMs: number; entries: number; velocity: [number, number] }

/// x right, y up.
const DIRECTIONS: Record<string, [number, number]> = {
  ArrowRight: [1, 0],
  KeyD: [1, 0],
  ArrowLeft: [-1, 0],
  KeyA: [-1, 0],
  ArrowUp: [0, 1],
  KeyW: [0, 1],
  ArrowDown: [0, -1],
  KeyS: [0, -1]
}

export function isDriveKey(code: string): boolean {
  return code in DIRECTIONS
}

/// `speed` is in units per second.
export function velocityOf(held: Iterable<string>, speed: number): [number, number] {
  let vx = 0
  let vy = 0
  for (const code of held) {
    const direction = DIRECTIONS[code]
    if (direction) {
      vx += direction[0]
      vy += direction[1]
    }
  }
  return [Math.sign(vx) * speed, Math.sign(vy) * speed]
}

/// One `action` record answering the state of robot time `basis`.
export function encodeAction(chunk: ActionChunk, nowMs: number): Uint8Array {
  const record = {
    T: Math.round(nowMs),
    data: {
      seq: chunk.seq,
      basis: chunk.basis,
      dt_ms: chunk.dtMs,
      actions: Array.from({ length: chunk.entries }, () => chunk.velocity)
    }
  }
  return new TextEncoder().encode(JSON.stringify([record]))
}

function lastRecord(payload: Uint8Array): { T: number; data: unknown } | undefined {
  try {
    const records: unknown = JSON.parse(new TextDecoder().decode(payload))
    if (!Array.isArray(records) || records.length === 0) {
      return undefined
    }
    const record = records[records.length - 1] as { T?: unknown; data?: unknown }
    return typeof record.T === 'number' ? { T: record.T, data: record.data } : undefined
  } catch {
    return undefined
  }
}

export function decodeState(payload: Uint8Array): RobotState | undefined {
  const record = lastRecord(payload)
  const data = record?.data as { x?: unknown; y?: unknown } | undefined
  if (!record || typeof data?.x !== 'number' || typeof data.y !== 'number') {
    return undefined
  }
  return { t: record.T, x: data.x, y: data.y }
}

export function decodeControl(payload: Uint8Array): Control | undefined {
  const data = lastRecord(payload)?.data as { driver?: unknown; reason?: unknown } | undefined
  if (typeof data?.reason !== 'string' || (data.driver !== null && typeof data.driver !== 'string')) {
    return undefined
  }
  return { driver: data.driver ?? null, reason: data.reason }
}
