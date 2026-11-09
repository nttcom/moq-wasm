import { init } from 'moq-wasm'
import type { LiveStats } from 'moq-wasm/media'
import { ClientTelemetry, type StatsSample } from './clientTelemetry'
import { resolveServiceUrl } from './serviceUrl'

const TELEMETRY_URLS = {
  param: 'telemetry',
  local: 'http://localhost:8095',
  cloud: 'https://observability.moqt.research.skyway.io'
}
const FLUSH_INTERVAL_MS = 10_000

/// Every `FLUSH_INTERVAL_MS`, sends `page`'s wasm logs (`info` and above) and the stats `samples` returns.
/// `relayUrl` picks the local or deployed observability server, as `resolveServiceUrl` does.
export function startPageTelemetry(
  page: string,
  relayUrl: () => string,
  samples: () => StatsSample[] = () => []
): ClientTelemetry {
  const telemetry = new ClientTelemetry(
    page,
    () => `${resolveServiceUrl(location.search, relayUrl(), TELEMETRY_URLS)}/api/client-telemetry`
  )
  init({ onLog: telemetry.wasmLog, onSpan: telemetry.wasmSpan, logLevel: 'info' }).catch((error) =>
    telemetry.log('error', `wasm init: ${String(error)}`)
  )
  setInterval(() => {
    samples().forEach((sample) => telemetry.sample(sample))
    telemetry.flush()
  }, FLUSH_INTERVAL_MS)
  addEventListener('pagehide', () => telemetry.flush(true))
  return telemetry
}

export function playerSamples(player: { readonly state: { started: boolean }; stats(): LiveStats }): StatsSample[] {
  return player.state.started ? [{ values: player.stats() }] : []
}
