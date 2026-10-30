import { MoqSession } from 'moq-wasm'
import type { TrackWriter } from 'moq-wasm'
import { LivePlayer } from 'moq-wasm/media/player/livePlayer'
import { formatLiveStats } from 'moq-wasm/media/player/locLive'
import { PlayerControls } from 'moq-wasm/media/controls/playerControls'
import { setUpRelayUrlControls } from '../../utils/relayPresets'
import { element, parseTrackNamespace } from '../../utils/page'
import 'moq-wasm/media/player.css'
import 'moq-wasm/media/controls.css'

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

const DEFAULT_VALUES: Record<FieldKey, number> = {
  pan: 0,
  tilt: 0,
  zoom: 0,
  speed: 1
}

let session: MoqSession | undefined
const stage = element<HTMLDivElement>('stage')
const player = new LivePlayer({
  container: stage,
  callbacks: { onStateChange: renderPlayer, onLiveFrame: renderVideoStats, onLog: log }
})
const controls = new PlayerControls(stage, player, log)
let commandWriter: TrackWriter | undefined

function renderPlayer(): void {
  const { catalogStatus, playbackStatus, rewindStatus } = player.state
  element('playback-status').textContent = `${playbackStatus.text} · ${rewindStatus.text} · ${catalogStatus.text}`
  controls.render()
}

function renderVideoStats(): void {
  const stats = player.stats()
  element('video-stats').textContent = formatLiveStats(stats)
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

function updateCommandTrack(state: string): void {
  element('command-track-state').textContent = state
}

function inputValue(id: string): string {
  return element<HTMLInputElement>(id).value
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
  if (!commandWriter) {
    updateStatus('not connected', false)
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
  if ((await commandWriter.writeGroup(bytes)) === undefined) {
    updateCommandTrack('not subscribed')
    updateStatus('command track not subscribed', false)
    return
  }
  updateCommandTrack('subscribed')
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
  setUpRelayUrlControls(element('moqt-url'), element('urlPresets'))
}

async function connect(): Promise<void> {
  const url = inputValue('moqt-url')
  const publishNamespace = parseTrackNamespace(inputValue('publish-namespace'))
  const subscribeNamespace = parseTrackNamespace(inputValue('subscribe-namespace'))
  const commandTrack = inputValue('command-track')

  if (!publishNamespace.length || !subscribeNamespace.length) {
    updateStatus('namespace required', false)
    return
  }

  await disconnect()
  updateStatus('connecting', true)
  const connected = await MoqSession.connect(url)
  session = connected
  void connected.closed.then(() => {
    if (session === connected) {
      updateStatus('disconnected', false)
    }
  })
  await connected.announce(publishNamespace)
  // Only the newest command matters to a camera that subscribes later.
  commandWriter = await connected.publishTrack(publishNamespace, commandTrack, { retentionMs: 0 })
  updateCommandTrack('published')
  updateStatus('connected', true)
  await player.start(connected, subscribeNamespace)
}

async function disconnect(): Promise<void> {
  await player.stop()
  const closing = session
  session = undefined
  commandWriter = undefined
  closing?.close()
  updateCommandTrack('-')
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

const settingsModal = element<HTMLDialogElement>('settings-modal')
element('settings-btn').addEventListener('click', () => settingsModal.showModal())
settingsModal.addEventListener('click', (event) => {
  if (event.target === settingsModal) {
    settingsModal.close()
  }
})

buildCommandUI()
setupUrlPresets()
