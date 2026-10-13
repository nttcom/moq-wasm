import { element } from '../../utils/page'
import { setUpRelayUrlControls } from '../../utils/relayPresets'
import type { MoqtClientWrapper } from '@moqt/moqtClient'
import type { MOQTClient } from '../../pkg/moqt'

export type MediaVideoHardwareAcceleration = 'prefer-hardware' | 'prefer-software' | 'no-preference'

export type MediaVideoEncodingOverrides = {
  codec?: string
  hardwareAcceleration?: MediaVideoHardwareAcceleration
}

export function requireRawClient(moqtClient: MoqtClientWrapper): MOQTClient {
  const client = moqtClient.getRawClient()
  if (!client) {
    throw new Error('MOQT client not connected')
  }
  return client
}

export function initializeMediaExamplePage(namespaceInputId: string): void {
  setUpRelayUrlControls(element('url'), element('urlPresets'))
  applyNamespaceOverride(namespaceInputId)
}

export function getMediaVideoEncodingOverrides(): MediaVideoEncodingOverrides {
  const params = new URLSearchParams(window.location.search)
  const codec = firstNonEmptyValue(params, ['videoCodec'])
  const hardwareAcceleration = parseMediaVideoHardwareAcceleration(
    firstNonEmptyValue(params, ['videoEncoderHardwareAcceleration', 'videoHardwareAcceleration'])
  )
  return {
    codec: codec ?? undefined,
    hardwareAcceleration
  }
}

function applyNamespaceOverride(namespaceInputId: string): void {
  const input = document.getElementById(namespaceInputId)
  if (!(input instanceof HTMLInputElement)) {
    return
  }

  const params = new URLSearchParams(window.location.search)
  input.value =
    firstNonEmptyValue(params, ['trackNamespace', 'namespace']) ?? (input.value.trim() || buildDefaultTrackNamespace())
}

export function buildDefaultTrackNamespace(date = new Date()): string {
  const pad = (value: number) => String(value).padStart(2, '0')
  const day = `${date.getFullYear()}${pad(date.getMonth() + 1)}${pad(date.getDate())}`
  return `anon/nttcom/${day}/${pad(date.getHours())}${pad(date.getMinutes())}`
}

function firstNonEmptyValue(params: URLSearchParams, keys: string[]): string | null {
  for (const key of keys) {
    const value = params.get(key)?.trim()
    if (value) {
      return value
    }
  }
  return null
}

function parseMediaVideoHardwareAcceleration(value: string | null): MediaVideoHardwareAcceleration | undefined {
  if (value === 'prefer-hardware' || value === 'prefer-software' || value === 'no-preference') {
    return value
  }
  return undefined
}
