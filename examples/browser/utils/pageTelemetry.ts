import { init } from 'moq-wasm'
import { resolveServiceUrl } from './serviceUrl'

const TELEMETRY_URLS = {
  param: 'telemetry',
  local: 'http://localhost:8095',
  cloud: 'https://observability.moqt.research.skyway.io'
}

/// Has the SDK send `page`'s wasm logs (`info` and above), spans and players' stats every 10 s.
/// `relayUrl` picks the local or deployed observability server, as `resolveServiceUrl` does.
export function startPageTelemetry(page: string, relayUrl: () => string): void {
  init({
    logLevel: 'info',
    telemetry: { page, url: () => resolveServiceUrl(location.search, relayUrl(), TELEMETRY_URLS) }
  }).catch((error) => console.error(`[${page}] wasm init failed`, error))
}
