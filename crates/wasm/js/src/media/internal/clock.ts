const MICROS_PER_MILLISECOND = 1000

/// The clock of the LOC capture timestamps `createBroadcast` writes and of `Playhead.captureMicros`.
export function monotonicUnixMicros(): number {
  return Math.round((performance.timeOrigin + performance.now()) * MICROS_PER_MILLISECOND)
}

export function latencyMsFromCaptureMicros(captureTimestampMicros: number): number {
  return (monotonicUnixMicros() - captureTimestampMicros) / MICROS_PER_MILLISECOND
}
