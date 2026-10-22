import type { BufferPolicy } from '@player/jitterBuffer'
import { type CatchUp, DEFAULT_BUFFER_POLICY } from '@player/livePlayout'

export type PlayoutSettings = {
  policy: BufferPolicy
  catchUp: CatchUp
}

/// The buffer is then the measured jitter plus the output latency. A lower
/// rendition spreads keyframes less, so stepping down shortens the shown
/// delay that the quality choice holds to its target.
export const DEFAULT_PLAYOUT_SETTINGS: PlayoutSettings = {
  policy: { ...DEFAULT_BUFFER_POLICY, minimumMs: 0 },
  catchUp: 'speed-up'
}
