import type { BufferPolicy } from '@player/jitterBuffer'
import { type CatchUp, DEFAULT_BUFFER_POLICY } from '@player/livePlayout'

export type PlayoutSettings = {
  policy: BufferPolicy
  catchUp: CatchUp
}

export const DEFAULT_PLAYOUT_SETTINGS: PlayoutSettings = {
  policy: DEFAULT_BUFFER_POLICY,
  catchUp: 'skip'
}
