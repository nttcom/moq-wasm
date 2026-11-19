import { urlFromQuery } from './serviceUrl'

const DEFAULT_LOCAL_RELAY_A_URL = 'https://127.0.0.1:4433'
const DEFAULT_LOCAL_RELAY_B_URL = 'https://127.0.0.1:4434'

export type RelayPreset = {
  label: string
  value: string
  helper: string
  testId?: string
}

export const CLOUD_RELAY_PRESETS: RelayPreset[] = [1, 2, 3].map((n) => {
  const host = `relay-${n}.moqt.research.skyway.io:443`
  return { label: `relay-${n}`, value: `https://${host}`, helper: host }
})

export const LOAD_BALANCED_RELAY_PRESET: RelayPreset = {
  label: 'relay.moqt.research.skyway.io',
  value: 'https://relay.moqt.research.skyway.io:443',
  helper: 'Load-balanced cloud relay'
}

const RELAY_URL_PARAM_NAMES = ['relayUrl', 'moqtUrl', 'url', 'relay']

export function relayUrlFromQuery(): string | undefined {
  return urlFromQuery(window.location.search, RELAY_URL_PARAM_NAMES)
}

export function firstSearchParam(paramNames: string[]): string | null {
  const params = new URLSearchParams(window.location.search)
  for (const name of paramNames) {
    const value = params.get(name)?.trim()
    if (value) {
      return value
    }
  }
  return null
}

export function buildLocalRelayPresets(): RelayPreset[] {
  const relayAUrl = urlFromQuery(window.location.search, ['relayAUrl']) ?? DEFAULT_LOCAL_RELAY_A_URL
  const relayBUrl = urlFromQuery(window.location.search, ['relayBUrl']) ?? DEFAULT_LOCAL_RELAY_B_URL
  return [
    {
      label: 'relay-a',
      value: relayAUrl,
      helper: `Docker compose relay-a (${relayAUrl})`,
      testId: 'join-relay-a-radio'
    },
    {
      label: 'relay-b',
      value: relayBUrl,
      helper: `Docker compose relay-b (${relayBUrl})`,
      testId: 'join-relay-b-radio'
    }
  ]
}
