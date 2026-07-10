export const DEFAULT_CLOUD_RELAY_URL = 'https://relay-1.moqt.research.skyway.io:443'
export const DEFAULT_LOCAL_RELAY_A_URL = 'https://127.0.0.1:4433'
export const DEFAULT_LOCAL_RELAY_B_URL = 'https://127.0.0.1:4434'

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

const DEFAULT_RELAY_URL_PARAM_NAMES = ['relayUrl', 'moqtUrl', 'url', 'relay'] as const

type RelayUrlControlsOptions = {
  inputId?: string
  presetContainerId?: string
  input?: HTMLInputElement | null
  presetContainer?: HTMLElement | null
  defaultUrl?: string
  queryParamNames?: readonly string[]
  includeRelayB?: boolean
}

export function getLocalRelayUrls(): { relayAUrl: string; relayBUrl: string } {
  return {
    relayAUrl: readRelayUrlSearchParam(['relayAUrl']) ?? DEFAULT_LOCAL_RELAY_A_URL,
    relayBUrl: readRelayUrlSearchParam(['relayBUrl']) ?? DEFAULT_LOCAL_RELAY_B_URL
  }
}

export function getPrimaryRelayUrl(fallback = DEFAULT_CLOUD_RELAY_URL): string {
  return readRelayUrlSearchParam(DEFAULT_RELAY_URL_PARAM_NAMES) ?? fallback
}

export function readRelayUrlSearchParam(paramNames: readonly string[]): string | null {
  const params = getSearchParams()
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

export function configureRelayUrlControls({
  inputId = 'url',
  presetContainerId = 'urlPresets',
  input = getInput(inputId),
  presetContainer = document.getElementById(presetContainerId),
  defaultUrl = DEFAULT_CLOUD_RELAY_URL,
  queryParamNames = DEFAULT_RELAY_URL_PARAM_NAMES,
  includeRelayB = true
}: RelayUrlControlsOptions = {}): void {
  if (!input) {
    return
  }

  input.value = readRelayUrlSearchParam(queryParamNames) ?? (input.value.trim() || defaultUrl)

  if (!presetContainer) {
    return
  }

  const { relayAUrl, relayBUrl } = getLocalRelayUrls()
  upsertLocalRelayButton(presetContainer, {
    presetId: 'relay-a',
    fallbackValue: DEFAULT_LOCAL_RELAY_A_URL,
    label: 'Local relay-a',
    value: relayAUrl
  })
  if (includeRelayB) {
    upsertLocalRelayButton(presetContainer, {
      presetId: 'relay-b',
      fallbackValue: DEFAULT_LOCAL_RELAY_B_URL,
      label: 'Local relay-b',
      value: relayBUrl
    })
  }

  presetContainer.addEventListener('click', (event) => {
    const target = event.target
    if (!(target instanceof Element)) {
      return
    }
    const button = target.closest<HTMLButtonElement>('button[data-url]')
    const value = button?.dataset.url?.trim()
    if (!value) {
      return
    }
    input.value = value
  })
}

function upsertLocalRelayButton(
  container: HTMLElement,
  {
    presetId,
    fallbackValue,
    label,
    value
  }: {
    presetId: string
    fallbackValue: string
    label: string
    value: string
  }
): void {
  let button = container.querySelector<HTMLButtonElement>(`button[data-relay-preset="${presetId}"]`)
  if (!button) {
    button =
      Array.from(container.querySelectorAll<HTMLButtonElement>('button[data-url]')).find(
        (candidate) => candidate.dataset.url === fallbackValue
      ) ?? null
  }
  if (!button) {
    button = document.createElement('button')
    button.type = 'button'
    button.className = firstPresetButtonClassName(container)
    container.appendChild(button)
  }
  button.dataset.relayPreset = presetId
  button.dataset.url = value
  button.textContent = `${label}: ${value}`
}

function getInput(inputId: string): HTMLInputElement | null {
  const input = document.getElementById(inputId)
  return input instanceof HTMLInputElement ? input : null
}

function getSearchParams(): URLSearchParams {
  if (typeof window === 'undefined') {
    return new URLSearchParams()
  }
  return new URLSearchParams(window.location.search)
}

function firstPresetButtonClassName(container: HTMLElement): string {
  return container.querySelector<HTMLButtonElement>('button')?.className ?? ''
}
