import { init } from 'moq-wasm'
import type { LivePlayerStats } from 'moq-wasm/media/player/livePlayer'
import { ClientTelemetry } from './clientTelemetry'
import { resolveServiceUrl } from './serviceUrl'

const TELEMETRY_URLS = {
  param: 'telemetry',
  local: 'http://localhost:8095',
  cloud: 'https://observability.moqt.research.skyway.io'
}
const FLUSH_INTERVAL_MS = 10_000

/// Starts sending `page`'s wasm logs and, while it plays, `player`'s stats every `FLUSH_INTERVAL_MS`.
/// `player` is a getter so a page can start telemetry before it builds its player.
/// `relayUrl` picks the local or deployed observability server, as `resolveServiceUrl` does.
export function startPageTelemetry(
  page: string,
  relayUrl: () => string,
  player?: () => { readonly state: { started: boolean }; stats(): LivePlayerStats }
): ClientTelemetry {
  const telemetry = new ClientTelemetry(
    page,
    () => `${resolveServiceUrl(location.search, relayUrl(), TELEMETRY_URLS)}/api/client-telemetry`
  )
  init({ onLog: telemetry.wasmLog }).catch((error) => telemetry.log('error', `wasm init: ${String(error)}`))
  setInterval(() => {
    const playing = player?.()
    if (playing?.state.started) {
      telemetry.sample(playing.stats())
    }
    telemetry.flush()
  }, FLUSH_INTERVAL_MS)
  addEventListener('pagehide', () => telemetry.flush(true))
  return telemetry
}
