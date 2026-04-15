export const DEFAULT_MOQT_URL = 'https://moqt.research.skyway.io:4433'

export function initializeMediaExamplePage(namespaceInputId: string): void {
  bindUrlPresetButtons()
  applyUrlOverride()
  applyNamespaceOverride(namespaceInputId)
}

export function parseTrackNamespace(value: string): string[] {
  return value
    .split('/')
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
}

export function setStatusText(id: string, text: string): void {
  const element = document.getElementById(id)
  if (!(element instanceof HTMLElement)) {
    return
  }

  const normalized = text.trim()
  element.textContent = normalized
  element.style.display = normalized.length > 0 ? '' : 'none'
}

export function getErrorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message
  }
  if (typeof error === 'string') {
    return error
  }
  try {
    return JSON.stringify(error)
  } catch (_error) {
    return String(error)
  }
}

function bindUrlPresetButtons(): void {
  const preset = document.getElementById('urlPresets')
  const input = document.getElementById('url')
  if (!(preset instanceof HTMLElement) || !(input instanceof HTMLInputElement)) {
    return
  }

  preset.addEventListener('click', (event) => {
    const target = event.target
    if (!(target instanceof HTMLButtonElement)) {
      return
    }
    const value = target.getAttribute('data-url')
    if (!value) {
      return
    }
    input.value = value
  })
}

function applyUrlOverride(): void {
  const input = document.getElementById('url')
  if (!(input instanceof HTMLInputElement)) {
    return
  }

  const params = new URLSearchParams(window.location.search)
  input.value = firstNonEmptyValue(params, ['moqtUrl', 'url']) ?? (input.value.trim() || DEFAULT_MOQT_URL)
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

function buildDefaultTrackNamespace(date = new Date()): string {
  const year = date.getFullYear()
  const month = pad(date.getMonth() + 1)
  const day = pad(date.getDate())
  const hours = pad(date.getHours())
  const minutes = pad(date.getMinutes())
  return `nttcom/${year}${month}${day}/${hours}${minutes}`
}

function pad(value: number): string {
  return value < 10 ? `0${value}` : String(value)
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
