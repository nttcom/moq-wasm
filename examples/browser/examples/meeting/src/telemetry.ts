import { startPageTelemetry } from '../../../utils/pageTelemetry'
import type { LocalSession } from './session/localSession'

let session: LocalSession | undefined

startPageTelemetry('meeting', () => session?.relayUrl ?? '')

/// Points the telemetry at the observability server of `joined`'s relay until the returned function is called.
export function reportSession(joined: LocalSession): () => void {
  session = joined
  return () => {
    session = undefined
  }
}
