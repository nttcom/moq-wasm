const MICROS_PER_MILLISECOND = 1000

/// The clock of the LOC capture timestamps `publish()` writes and of `Playhead.captureMicros`.
export function monotonicUnixMicros(): number {
  return Math.round((performance.timeOrigin + performance.now()) * MICROS_PER_MILLISECOND)
}

export function latencyMsFromCaptureMicros(captureTimestampMicros: number): number {
  return (monotonicUnixMicros() - captureTimestampMicros) / MICROS_PER_MILLISECOND
}

/// The relay isolates a track whose publisher repeats a location, so a
/// publisher that comes back must not restart its group ids at zero.
export function timeSeededGroupId(): bigint {
  return BigInt(monotonicUnixMicros())
}
