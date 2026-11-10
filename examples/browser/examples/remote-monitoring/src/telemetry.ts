import type { TelemetryStatsSample } from 'moq-wasm'
import { startPageTelemetry } from '../../../utils/pageTelemetry'

let relayUrl = ''
let samples: () => TelemetryStatsSample[] = () => []

startPageTelemetry(
  'remote-monitoring',
  () => relayUrl,
  () => samples()
)

/// Picks the observability server from the relay at `url`.
export function reportRelay(url: string): void {
  relayUrl = url
}

export function sampleCameras(next: () => TelemetryStatsSample[]): void {
  samples = next
}
