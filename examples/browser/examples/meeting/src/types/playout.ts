import type { BufferPolicy, CatchUp } from 'moq-wasm/media'

export type PlayoutSettings = {
  policy: BufferPolicy
  catchUp: CatchUp
}

/// The buffer is the measured jitter plus the output latency. The jitter
/// leaves out the slowest 5% of arrivals: a conversation suffers more from a
/// second of delay than from a rare late arrival, which is concealed. A lower
/// rendition spreads keyframes less, so stepping down shortens the shown delay
/// that the quality choice holds to its target.
export const DEFAULT_PLAYOUT_SETTINGS: PlayoutSettings = {
  policy: { minimumMs: 0, maximumMs: Number.POSITIVE_INFINITY, arrivalQuantile: 0.95 },
  catchUp: 'speed-up'
}
