import { MoqSession, reportLog } from 'moq-wasm'
import { connectOptions } from '../../utils/access'
import type { TrackWriter } from 'moq-wasm'
import { playerSamples, startPageTelemetry } from '../../utils/pageTelemetry'
import { LivePlayer, PlayerControls, formatLiveStats } from 'moq-wasm/media'
import { LOAD_BALANCED_RELAY_PRESET, relayUrlFromQuery } from '../../utils/relayPresets'
import { createRelayPicker } from '../../utils/relayPicker'
import { element, getErrorMessage, parseTrackNamespace } from '../../utils/page'
import 'moq-wasm/media/player.css'
import 'moq-wasm/media/controls.css'

type CommandKind = 'absolute' | 'relative' | 'continuous' | 'stop' | 'center'

type FieldKey = 'pan' | 'tilt' | 'zoom' | 'speed'

type CommandPayload =
  | { type: 'stop' }
  | { type: 'center'; speed: number }
  | { type: 'absolute' | 'relative' | 'continuous'; pan: number; tilt: number; zoom: number; speed: number }

const MOVES: Record<string, { pan: number; tilt: number }> = {
  up: { pan: 0, tilt: 1 },
  down: { pan: 0, tilt: -1 },
  left: { pan: -1, tilt: 0 },
  right: { pan: 1, tilt: 0 }
}

const CONTROLLER_SPEED = 0.5

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
let relayUrl = relayUrlFromQuery() ?? LOAD_BALANCED_RELAY_PRESET.value
const stage = element<HTMLDivElement>('stage')
startPageTelemetry(
  'onvif',
  () => relayUrl,
  () => playerSamples(player)
)
const player = new LivePlayer({
  container: stage,
  callbacks: { onStateChange: renderPlayer, onLiveFrame: renderVideoStats, onLog: log }
})
const controls = new PlayerControls(stage, player, log)
let commandWriter: TrackWriter | undefined
// Commands go out one at a time so a Stop never overtakes the move it ends.
let commandQueue: Promise<void> = Promise.resolve()
let activeMove: string | null = null

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
  reportLog(level, 'onvif', message)
  console[level](`[onvif] ${message}`)
}

function updateStatus(id: 'status' | 'command-state', label: string, ok: boolean): void {
  const pill = element(id)
  pill.innerHTML = `<span></span>${label}`
  const dot = pill.querySelector('span')
  if (dot instanceof HTMLElement) {
    dot.style.background = ok ? '#2ec4b6' : '#ff9f1c'
  }
}

function showLiveView(live: boolean): void {
  element<HTMLElement>('connect-view').hidden = live
  element<HTMLElement>('live-view').hidden = !live
}

function showConnectError(message: string | null): void {
  const error = element<HTMLElement>('connect-error')
  error.textContent = message ?? ''
  error.hidden = message === null
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

function enqueueCommand(payload: CommandPayload): void {
  commandQueue = commandQueue
    .then(() => sendPayload(payload))
    .catch((err) => {
      console.error(err)
      updateStatus('status', 'command send failed', false)
    })
}

async function sendPayload(payload: CommandPayload): Promise<void> {
  if (!commandWriter) {
    updateStatus('status', 'not connected', false)
    return
  }
  const bytes = new TextEncoder().encode(JSON.stringify(payload))
  if ((await commandWriter.writeGroup(bytes)) === undefined) {
    updateStatus('command-state', 'camera not subscribed', false)
    return
  }
  updateStatus('status', `sent ${payload.type}`, true)
}

function sendCommand(command: CommandKind, row: HTMLElement): void {
  const pan = readField(row, 'pan')
  const tilt = readField(row, 'tilt')
  const zoom = readField(row, 'zoom')
  const speed = readField(row, 'speed')

  const payload: CommandPayload =
    command === 'stop'
      ? { type: 'stop' }
      : command === 'center'
        ? { type: 'center', speed }
        : { type: command, pan, tilt, zoom, speed }
  enqueueCommand(payload)
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
    button.addEventListener('click', () => sendCommand(command.type, row))

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

function pressMove(move: string): void {
  if (activeMove === move || !commandWriter) return
  activeMove = move
  enqueueCommand({ type: 'continuous', ...MOVES[move], zoom: 0, speed: CONTROLLER_SPEED })
}

function releaseMove(move: string | null = activeMove): void {
  if (activeMove === null || activeMove !== move) return
  activeMove = null
  if (commandWriter) enqueueCommand({ type: 'stop' })
}

function setUpController(): void {
  document.querySelectorAll<HTMLButtonElement>('[data-move]').forEach((button) => {
    const move = button.dataset.move ?? ''
    button.addEventListener('pointerdown', (event) => {
      event.preventDefault()
      button.setPointerCapture(event.pointerId)
      pressMove(move)
    })
    button.addEventListener('lostpointercapture', () => releaseMove(move))
  })

  element('center-btn').addEventListener('click', () => {
    releaseMove()
    enqueueCommand({ type: 'center', speed: CONTROLLER_SPEED })
  })

  window.addEventListener('blur', () => releaseMove())
}

function setUpTabs<T extends string>(tablist: HTMLElement, onSelect: (tab: T) => void): (tab: T) => void {
  const select = (tab: T) => {
    tablist.querySelectorAll<HTMLButtonElement>('[role="tab"]').forEach((button) => {
      button.setAttribute('aria-selected', String(button.dataset.tab === tab))
    })
    onSelect(tab)
  }
  tablist.querySelectorAll<HTMLButtonElement>('[role="tab"]').forEach((button) => {
    button.addEventListener('click', () => select(button.dataset.tab as T))
  })
  return select
}

function setUpPtzTabs(): void {
  setUpTabs<'controller' | 'commands'>(element('ptz-tabs'), (tab) => {
    releaseMove()
    element<HTMLElement>('ptz-panel-controller').hidden = tab !== 'controller'
    element<HTMLElement>('ptz-panel-commands').hidden = tab !== 'commands'
  })
}

async function connect(): Promise<void> {
  const url = relayUrl
  const publishNamespace = parseTrackNamespace(inputValue('publish-namespace'))
  const subscribeNamespace = parseTrackNamespace(inputValue('subscribe-namespace'))
  const commandTrack = inputValue('command-track')

  if (!publishNamespace.length || !subscribeNamespace.length) {
    showConnectError('Set the namespaces in Advanced settings.')
    return
  }

  await disconnect()
  const connected = await MoqSession.connect(url, connectOptions())
  session = connected
  void connected.closed.then(() => {
    if (session === connected) {
      void disconnect().then(() => showConnectError('The relay closed the connection.'))
    }
  })
  await connected.announce(publishNamespace)
  // Only the newest command matters to a camera that subscribes later.
  const writer = await connected.publishTrack(publishNamespace, commandTrack, { retentionMs: 0 })
  commandWriter = writer
  element<HTMLFieldSetElement>('ptz-controls').disabled = false
  void followCameraSubscriptions(writer)
  element('live-relay').textContent = url
  showLiveView(true)
  updateStatus('status', 'connected', true)
  await player.start(connected, subscribeNamespace)
}

async function followCameraSubscriptions(writer: TrackWriter): Promise<void> {
  while (await writer.nextSubscriber()) {
    if (commandWriter === writer) updateStatus('command-state', 'ready', true)
  }
}

async function disconnect(): Promise<void> {
  releaseMove()
  await commandQueue
  await player.stop()
  const closing = session
  session = undefined
  commandWriter = undefined
  closing?.close()
  element<HTMLFieldSetElement>('ptz-controls').disabled = true
  updateStatus('command-state', 'waiting for camera', false)
  showLiveView(false)
  showConnectError(null)
}

const connectButton = element<HTMLButtonElement>('connect-btn')
connectButton.addEventListener('click', async () => {
  connectButton.disabled = true
  connectButton.textContent = 'Connecting…'
  try {
    await connect()
  } catch (err) {
    console.error(err)
    if (element<HTMLElement>('live-view').hidden) {
      await disconnect().catch(console.error)
      showConnectError(`Connection failed: ${getErrorMessage(err)}`)
    } else {
      updateStatus('status', 'playback failed', false)
    }
  } finally {
    connectButton.disabled = false
    connectButton.textContent = 'Connect'
  }
})

element('disconnect-btn').addEventListener('click', () => {
  disconnect().catch((err) => {
    console.error(err)
  })
})

buildCommandUI()
createRelayPicker(element('relay-picker'), relayUrl, (url) => {
  relayUrl = url
})
setUpPtzTabs()
setUpController()
