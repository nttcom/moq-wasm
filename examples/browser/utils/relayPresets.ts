const DEFAULT_CLOUD_RELAY_URL = 'https://relay-1.moqt.research.skyway.io:443'
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

function getLocalRelayUrls(): { relayAUrl: string; relayBUrl: string } {
  return {
    relayAUrl: readRelayUrlSearchParam(['relayAUrl']) ?? DEFAULT_LOCAL_RELAY_A_URL,
    relayBUrl: readRelayUrlSearchParam(['relayBUrl']) ?? DEFAULT_LOCAL_RELAY_B_URL
  }
}

function readRelayUrlSearchParam(paramNames: string[]): string | null {
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

export function buildRelayPresets(): RelayPreset[] {
  return [...buildLocalRelayPresets(), LOAD_BALANCED_RELAY_PRESET, ...CLOUD_RELAY_PRESETS]
}

export function setUpRelayUrlControls(input: HTMLInputElement, presets: HTMLElement): void {
  input.value = readRelayUrlSearchParam(RELAY_URL_PARAM_NAMES) ?? (input.value.trim() || DEFAULT_CLOUD_RELAY_URL)
  const { relayAUrl, relayBUrl } = getLocalRelayUrls()
  const buttons = [
    ...[LOAD_BALANCED_RELAY_PRESET, ...CLOUD_RELAY_PRESETS].map((preset) => ({
      text: `Cloud ${preset.label}`,
      url: preset.value,
      title: preset.helper
    })),
    { text: `Local relay-a: ${relayAUrl}`, url: relayAUrl, title: '' },
    { text: `Local relay-b: ${relayBUrl}`, url: relayBUrl, title: '' }
  ]
  for (const { text, url, title } of buttons) {
    const button = document.createElement('button')
    button.type = 'button'
    button.textContent = text
    button.title = title
    button.addEventListener('click', () => {
      input.value = url
    })
    presets.appendChild(button)
  }
}
