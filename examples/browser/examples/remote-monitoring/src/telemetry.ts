import { startPageTelemetry } from '../../../utils/pageTelemetry'

let relayUrl = ''

startPageTelemetry('remote-monitoring', () => relayUrl)

/// Picks the observability server from the relay at `url`.
export function reportRelay(url: string): void {
  relayUrl = url
}
