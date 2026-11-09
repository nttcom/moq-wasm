import { startPageTelemetry } from '../../../utils/pageTelemetry'
import { memberSamples } from './media/memberSamples'
import type { LocalSession } from './session/localSession'
import type { RemoteMember } from './types/member'

let room: { session: LocalSession; members: () => RemoteMember[] } | undefined

startPageTelemetry(
  'meeting',
  () => room?.session.relayUrl ?? '',
  () => (room ? memberSamples(room.session.getMediaController().subscriber, room.members()) : [])
)

/// Samples `session`'s remote members until the returned function is called.
export function sampleRoom(session: LocalSession, members: () => RemoteMember[]): () => void {
  room = { session, members }
  return () => {
    room = undefined
  }
}
