import { urlFromQuery } from './serviceUrl'

const DEFAULT_LOCAL_RELAY_A_URL = 'https://127.0.0.1:4433'
const DEFAULT_LOCAL_RELAY_B_URL = 'https://127.0.0.1:4434'

export type RelayPreset = {
  label: string
  value: string
  helper: string
  testId?: string
}

export const CLOUD_RELAY_PRESETS: RelayPreset[] = [
  {
    label: 'relay-1',
    value: 'https://relay-1.moqt.research.skyway.io:443',
    helper: 'relay-1.moqt.research.skyway.io:443'
  },
  {
    label: 'relay-2',
    value: 'https://relay-2.moqt.research.skyway.io:443',
    helper: 'relay-2.moqt.research.skyway.io:443'
  },
  {
    label: 'relay-3',
    value: 'https://relay-3.moqt.research.skyway.io:443',
    helper: 'relay-3.moqt.research.skyway.io:443'
  }
]

export const LOAD_BALANCED_RELAY_PRESET: RelayPreset = {
  label: 'relay.moqt.research.skyway.io',
  value: 'https://relay.moqt.research.skyway.io:443',
  helper: 'Load-balanced cloud relay'
}

const RELAY_URL_PARAM_NAMES = ['relayUrl', 'moqtUrl', 'url', 'relay']

export function relayUrlFromQuery(): string | undefined {
  return urlFromQuery(window.location.search, RELAY_URL_PARAM_NAMES)
}

function getLocalRelayUrls(): { relayAUrl: string; relayBUrl: string } {
  return {
    relayAUrl: urlFromQuery(window.location.search, ['relayAUrl']) ?? DEFAULT_LOCAL_RELAY_A_URL,
    relayBUrl: urlFromQuery(window.location.search, ['relayBUrl']) ?? DEFAULT_LOCAL_RELAY_B_URL
  }
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
  const { relayAUrl, relayBUrl } = getLocalRelayUrls()
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
