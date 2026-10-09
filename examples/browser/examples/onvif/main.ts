import { MoqtClientWrapper } from '@moqt/moqtClient'
import { LivePlayer, formatSyncOffset } from '@player/livePlayer'
import { PlayerControls } from '@player/ui/playerControls'
import type { MOQTClient } from '../../pkg/moqt'
import { OBJECT_STATUS_END_OF_GROUP } from '../../utils/media/objectStatus'
import { DEFAULT_LOCAL_RELAY_A_URL, configureRelayUrlControls } from '../../utils/relayPresets'

type CommandKind = 'absolute' | 'relative' | 'continuous' | 'stop' | 'center'

type FieldKey = 'pan' | 'tilt' | 'zoom' | 'speed'

const COMMANDS: Array<{ type: CommandKind; label: string; disabled?: FieldKey[] }> = [
  { type: 'absolute', label: 'AbsoluteMove' },
  { type: 'relative', label: 'RelativeMove' },
  { type: 'continuous', label: 'ContinuousMove' },
  { type: 'stop', label: 'Stop', disabled: ['pan', 'tilt', 'zoom', 'speed'] },
  { type: 'center', label: 'Center', disabled: ['pan', 'tilt', 'zoom'] }
]

const FIELD_LABELS: Record<FieldKey, string> = {
  pan: 'Pan',
  tilt: 'Tilt',
  zoom: 'Zoom',
  speed: 'Speed'
}

const SUBGROUP_ID = 0n
const PUBLISHER_PRIORITY = 0

const DEFAULT_VALUES: Record<FieldKey, number> = {
  pan: 0,
  tilt: 0,
  zoom: 0,
  speed: 1
}

const moqtClient = new MoqtClientWrapper()
const stage = element<HTMLDivElement>('stage')
const player = new LivePlayer({
  client: moqtClient,
  container: stage,
  callbacks: { onStateChange: renderPlayer, onLiveFrame: renderVideoStats, onLog: log }
})
const controls = new PlayerControls(stage, player, log)
let commandTrackAlias: bigint | null = null
/// The relay isolates a track whose publisher repeats a location, so a
/// publisher that comes back must not restart its group ids at zero.
let nextCommandGroupId = BigInt(Date.now()) * 1_000n

function renderPlayer(): void {
  const { catalogStatus, playbackStatus, rewindStatus } = player.state
  element('playback-status').textContent = `${playbackStatus.text} · ${rewindStatus.text} · ${catalogStatus.text}`
  controls.render()
}

function renderVideoStats(): void {
  const stats = player.stats()
  const size = stats.frameSize ? `${stats.frameSize.width}x${stats.frameSize.height}` : '-'
  const delay = stats.viewerDelayMs === undefined ? '-' : `${Math.round(stats.viewerDelayMs)} ms`
  const buffer = stats.bufferMs === undefined ? '-' : `${Math.round(stats.bufferMs)} ms`
  element('video-stats').textContent =
    `${size} · delay ${delay} · buffer ${buffer} · ${Math.round(stats.receivedKbps)} kbps · A/V ${formatSyncOffset(stats.syncOffsetMs)}`
  controls.renderBuffer(stats)
}

function log(level: 'info' | 'warn' | 'error', message: string): void {
  console[level](`[onvif] ${message}`)
}

function updateStatus(label: string, active: boolean): void {
  const status = element('status')
  status.innerHTML = `<span></span>${label}`
  const dot = status.querySelector('span')
  if (dot instanceof HTMLElement) {
    dot.style.background = active ? '#2ec4b6' : '#ff9f1c'
  }
}

function updateCommandAlias(alias: bigint | null): void {
  element('command-alias').textContent = alias === null ? '-' : alias.toString()
}

function ensureClient(): MOQTClient {
  const client = moqtClient.getRawClient()
  if (!client) {
    throw new Error('MOQT client not connected')
  }
  return client
}

function parseNamespace(value: string): string[] {
  return value
    .split('/')
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
}

function parseBigInt(value: string): bigint {
  try {
    return BigInt(value)
  } catch {
    return 0n
  }
}

function inputValue(id: string): string {
  return element<HTMLInputElement>(id).value
}

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id)
  if (!found) {
    throw new Error(`missing element: ${id}`)
  }
  return found as T
}

function readField(row: HTMLElement, field: FieldKey): number {
  const input = row.querySelector<HTMLInputElement>(`input[data-field="${field}"]`)
  if (!input) return DEFAULT_VALUES[field]
  const raw = Number(input.value)
  const value = Number.isFinite(raw) ? raw : DEFAULT_VALUES[field]
  const rounded = Math.round(value * 10) / 10
  input.value = rounded.toFixed(1)
  return rounded
}

async function sendCommand(command: CommandKind, row: HTMLElement): Promise<void> {
  const client = ensureClient()
  if (commandTrackAlias === null) {
    updateStatus('command track not subscribed', false)
    return
  }

  const pan = readField(row, 'pan')
  const tilt = readField(row, 'tilt')
  const zoom = readField(row, 'zoom')
  const speed = readField(row, 'speed')

  const payload =
    command === 'stop'
      ? { type: 'stop' }
      : command === 'center'
        ? { type: 'center', speed }
        : { type: command, pan, tilt, zoom, speed }

  const bytes = new TextEncoder().encode(JSON.stringify(payload))
  const groupId = nextCommandGroupId++
  await client.sendSubgroupHeader(commandTrackAlias, groupId, SUBGROUP_ID, PUBLISHER_PRIORITY)
  await client.sendSubgroupObject(commandTrackAlias, groupId, SUBGROUP_ID, 0n, undefined, bytes, undefined)
  await client.sendSubgroupObject(
    commandTrackAlias,
    groupId,
    SUBGROUP_ID,
    1n,
    OBJECT_STATUS_END_OF_GROUP,
    new Uint8Array(0),
    undefined
  )
  updateStatus(`sent ${command}`, true)
}

function buildCommandUI(): void {
  const grid = element('command-grid')
  grid.innerHTML = ''

  for (const command of COMMANDS) {
    const row = document.createElement('div')
    row.className = 'command'

    const header = document.createElement('div')
    header.className = 'command-header'

    const title = document.createElement('div')
    title.className = 'command-title'
    title.textContent = command.label

    const button = document.createElement('button')
    button.type = 'button'
    button.textContent = 'Send'
    button.addEventListener('click', () => {
      sendCommand(command.type, row).catch((err) => {
        console.error(err)
        updateStatus('command send failed', false)
      })
    })

    header.append(title, button)

    const inputs = document.createElement('div')
    inputs.className = 'command-inputs'

    const disabledFields = new Set(command.disabled ?? [])
    ;(Object.keys(FIELD_LABELS) as FieldKey[]).forEach((field) => {
      const label = document.createElement('label')
      label.textContent = FIELD_LABELS[field]

      const input = document.createElement('input')
      input.type = 'number'
      input.step = '0.1'
      input.inputMode = 'decimal'
      input.value = DEFAULT_VALUES[field].toFixed(1)
      input.dataset.field = field
      if (disabledFields.has(field)) {
        input.disabled = true
      }
      label.appendChild(input)
      inputs.appendChild(label)
    })

    row.append(header, inputs)
    grid.appendChild(row)
  }
}

function setupUrlPresets(): void {
  const urlInput = element<HTMLInputElement>('moqt-url')
  const presetContainer = urlInput.closest('form')?.querySelector<HTMLElement>('.button-row')
  configureRelayUrlControls({
    input: urlInput,
    presetContainer,
    defaultUrl: DEFAULT_LOCAL_RELAY_A_URL
  })
}

function openSettingsModal(): void {
  const modal = element('settings-modal')
  modal.classList.add('open')
  modal.setAttribute('aria-hidden', 'false')
}

function closeSettingsModal(): void {
  const modal = element('settings-modal')
  modal.classList.remove('open')
  modal.setAttribute('aria-hidden', 'true')
}

async function connect(): Promise<void> {
  const url = inputValue('moqt-url')
  const publishNamespace = parseNamespace(inputValue('publish-namespace'))
  const subscribeNamespace = parseNamespace(inputValue('subscribe-namespace'))
  const publishNamespaceLabel = publishNamespace.join('/')
  const commandTrack = inputValue('command-track')
  const authInfo = inputValue('auth-info')
  const maxRequestId = parseBigInt(inputValue('max-subscribe-id'))

  if (!publishNamespace.length || !subscribeNamespace.length) {
    updateStatus('namespace required', false)
    return
  }

  await disconnect()
  updateStatus('connecting', true)
  await moqtClient.connect(url, { maxRequestId })
  moqtClient.setOnConnectionClosedHandler(() => {
    updateStatus('disconnected', false)
  })

  moqtClient.setOnIncomingSubscribeHandler(async ({ subscribe, isSuccess, code, respondOk, respondError }) => {
    if (!isSuccess) {
      await respondError(BigInt(code), 'subscribe rejected')
      return
    }

    if (subscribe.trackName !== commandTrack || subscribe.trackNamespace.join('/') !== publishNamespaceLabel) {
      await respondError(404n, 'unknown track')
      return
    }

    commandTrackAlias = await respondOk(0n)
    updateCommandAlias(commandTrackAlias)
  })

  await moqtClient.publishNamespace(publishNamespace, authInfo)
  updateStatus('connected', true)
  await player.start(subscribeNamespace, authInfo)
}

async function disconnect(): Promise<void> {
  await player.stop()
  if (moqtClient.getConnectionStatus()) {
    await moqtClient.disconnect()
  }
  commandTrackAlias = null
  updateCommandAlias(commandTrackAlias)
  updateStatus('disconnected', false)
}

element('connect-btn').addEventListener('click', () => {
  connect().catch((err) => {
    console.error(err)
    updateStatus('connection failed', false)
  })
})

element('disconnect-btn').addEventListener('click', () => {
  disconnect().catch((err) => {
    console.error(err)
  })
})

const settingsModal = element('settings-modal')
element('settings-btn').addEventListener('click', openSettingsModal)
element('settings-close-btn').addEventListener('click', closeSettingsModal)
settingsModal.addEventListener('click', (event) => {
  if (event.target === settingsModal) {
    closeSettingsModal()
  }
})
document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && settingsModal.classList.contains('open')) {
    closeSettingsModal()
  }
})

buildCommandUI()
setupUrlPresets()
