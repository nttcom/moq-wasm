import './relayPicker.css'
import {
  CLOUD_RELAY_PRESETS,
  LOAD_BALANCED_RELAY_PRESET,
  buildLocalRelayPresets,
  relayUrlFromQuery,
  type RelayPreset
} from './relayPresets'

type RelayTab = 'cloud' | 'local'

const RELAY_TAB_LABELS: Record<RelayTab, string> = { cloud: 'Cloud', local: 'Local' }

export type RelayPicker = {
  /** Checks the option of `url` and opens its tab. A URL that matches no preset unchecks every option. */
  select(url: string): void
}

/**
 * `extra` (such as a URL input) is shown below the options while the picker is open. Switching tabs
 * selects the tab's first relay, so `onChange` also runs on a tab switch.
 */
export function createRelayPicker(
  container: HTMLElement,
  initialUrl: string,
  onChange: (url: string) => void,
  extra?: HTMLElement
): RelayPicker {
  const localPresets = buildLocalRelayPresets()
  const tabOf = (url: string): RelayTab | undefined => {
    if (localPresets.some((preset) => preset.value === url)) return 'local'
    if ([LOAD_BALANCED_RELAY_PRESET, ...CLOUD_RELAY_PRESETS].some((preset) => preset.value === url)) return 'cloud'
    return undefined
  }
  let selectedUrl = initialUrl
  let tab: RelayTab = tabOf(initialUrl) ?? 'cloud'

  const head = document.createElement('div')
  head.className = 'relay-picker-head'
  const title = document.createElement('span')
  title.className = 'relay-picker-title'
  title.textContent = 'Relay'
  const tablist = document.createElement('div')
  tablist.className = 'relay-picker-tabs'
  tablist.setAttribute('role', 'tablist')
  const tabButtons = (Object.keys(RELAY_TAB_LABELS) as RelayTab[]).map((value) => {
    const button = document.createElement('button')
    button.type = 'button'
    button.setAttribute('role', 'tab')
    button.dataset.testid = `relay-tab-${value}`
    button.textContent = RELAY_TAB_LABELS[value]
    button.addEventListener('click', () => {
      if (value === tab) return
      tab = value
      selectedUrl = value === 'cloud' ? LOAD_BALANCED_RELAY_PRESET.value : localPresets[0].value
      onChange(selectedUrl)
      render()
      renderSummary()
    })
    return { value, button }
  })
  tablist.append(...tabButtons.map(({ button }) => button))

  const summary = document.createElement('span')
  summary.className = 'relay-picker-summary'
  const toggle = document.createElement('button')
  toggle.type = 'button'
  toggle.className = 'relay-picker-toggle'
  toggle.dataset.testid = 'relay-picker-toggle'
  head.append(title, summary, toggle)

  const panel = document.createElement('div')
  panel.className = 'relay-picker-options'
  panel.setAttribute('role', 'tabpanel')

  const body = document.createElement('div')
  body.className = 'relay-picker-body'
  body.append(tablist, panel, ...(extra ? [extra] : []))

  let open = false
  const setOpen = (value: boolean) => {
    open = value
    body.hidden = !open
    toggle.textContent = open ? '閉じる' : '変更'
    toggle.setAttribute('aria-expanded', String(open))
  }
  toggle.addEventListener('click', () => setOpen(!open))
  setOpen(false)

  container.classList.add('relay-picker')
  container.replaceChildren(head, body)

  function renderSummary(): void {
    const preset = [LOAD_BALANCED_RELAY_PRESET, ...CLOUD_RELAY_PRESETS, ...localPresets].find(
      (candidate) => candidate.value === selectedUrl
    )
    const name = document.createElement('b')
    name.textContent = preset?.label ?? 'Custom'
    const url = document.createElement('code')
    url.textContent = selectedUrl
    summary.replaceChildren(name, url)
  }

  function option(preset: RelayPreset, variant: 'default' | 'child' | 'plain'): HTMLLabelElement {
    const label = document.createElement('label')
    label.className = variant === 'child' ? 'relay-picker-option compact' : 'relay-picker-option'

    const radio = document.createElement('input')
    radio.type = 'radio'
    radio.name = 'relay-picker'
    radio.value = preset.value
    radio.checked = preset.value === selectedUrl
    if (preset.testId) radio.dataset.testid = preset.testId
    radio.addEventListener('change', () => {
      selectedUrl = preset.value
      onChange(selectedUrl)
      renderSummary()
    })

    const name = document.createElement('span')
    name.className = 'relay-picker-option-label'
    name.textContent = preset.label
    if (variant === 'default') {
      const badge = document.createElement('span')
      badge.className = 'relay-picker-badge'
      badge.textContent = 'Default'
      name.append(badge)
    }
    const helper = document.createElement('span')
    helper.className = 'relay-picker-option-helper'
    helper.textContent = preset.helper
    const text = document.createElement('span')
    text.className = 'relay-picker-option-text'
    text.append(name, helper)

    label.append(radio, text)
    return label
  }

  function render(): void {
    for (const { value, button } of tabButtons) {
      button.setAttribute('aria-selected', String(value === tab))
    }
    if (tab === 'local') {
      panel.replaceChildren(...localPresets.map((preset) => option(preset, 'plain')))
      return
    }
    const children = document.createElement('div')
    children.className = 'relay-picker-children'
    children.append(...CLOUD_RELAY_PRESETS.map((preset) => option(preset, 'child')))
    panel.replaceChildren(option(LOAD_BALANCED_RELAY_PRESET, 'default'), children)
  }

  render()
  renderSummary()
  return {
    select(url: string) {
      selectedUrl = url
      tab = tabOf(url) ?? tab
      render()
      renderSummary()
    }
  }
}

/**
 * Renders the relay picker into `presets`, with `input` moved under its options, and syncs the two:
 * picking a relay fills `input`, and typing a URL checks its preset. The `relayUrl` query parameter overrides
 * the input's initial value.
 */
export function setUpRelayUrlControls(input: HTMLInputElement, presets: HTMLElement): void {
  input.value = relayUrlFromQuery() ?? (input.value.trim() || LOAD_BALANCED_RELAY_PRESET.value)
  const picker = createRelayPicker(
    presets,
    input.value,
    (url) => {
      input.value = url
    },
    input
  )
  input.addEventListener('input', () => picker.select(input.value.trim()))
}
