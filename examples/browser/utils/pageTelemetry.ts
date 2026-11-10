import { type TelemetryStatsSample, init } from 'moq-wasm'
import type { LiveStats } from 'moq-wasm/media'
import { resolveServiceUrl } from './serviceUrl'

const TELEMETRY_URLS = {
  param: 'telemetry',
  local: 'http://localhost:8095',
  cloud: 'https://observability.moqt.research.skyway.io'
}

/// Has the SDK send `page`'s wasm logs (`info` and above), spans and the stats `samples` returns
/// every 10 s. `relayUrl` picks the local or deployed observability server, as `resolveServiceUrl` does.
export function startPageTelemetry(
  page: string,
  relayUrl: () => string,
  samples: () => TelemetryStatsSample[] = () => []
): void {
  init({
    logLevel: 'info',
    telemetry: { page, url: () => resolveServiceUrl(location.search, relayUrl(), TELEMETRY_URLS), stats: samples }
  }).catch((error) => console.error(`[${page}] wasm init failed`, error))
}

export function playerSamples(player: {
  readonly state: { started: boolean }
  stats(): LiveStats
}): TelemetryStatsSample[] {
  return player.state.started ? [{ values: player.stats() }] : []
}
