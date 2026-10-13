import { setUpRelayUrlControls } from './relayPresets'

export type StatusState = 'idle' | 'ok' | 'review' | 'error'

export function setStatus(id: string, text: string, state: StatusState): void {
  const element = document.getElementById(id)
  if (element) {
    element.textContent = text
    element.dataset.state = state
  }
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

export function element<T extends Element>(id: string): T {
  const found = document.getElementById(id)
  if (!found) {
    throw new Error(`missing element: ${id}`)
  }
  return found as Element as T
}

export function parseTrackNamespace(value: string): string[] {
  return value
    .split('/')
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
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

export function initializeMediaExamplePage(namespaceInputId: string): void {
  setUpRelayUrlControls(element('url'), element('urlPresets'))
  applyNamespaceOverride(namespaceInputId)
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
