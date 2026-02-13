import { MoqtClientWrapper } from '@moqt/moqtClient'
import { parse_msf_catalog_json } from '../../pkg/moqt_client_wasm'
import type { MOQTClient } from '../../pkg/moqt_client_wasm'

const moqtClient = new MoqtClientWrapper()
const videoDecoderWorker = new Worker(new URL('../../utils/media/decoders/videoDecoder.ts', import.meta.url), {
  type: 'module'
})

type VideoDecoderWorkerMessage =
  | { type: 'frame'; frame: VideoFrame }
  | { type: 'bitrate'; kbps: number }
  | { type: 'receiveLatency'; media: 'video'; ms: number }
  | { type: 'renderingLatency'; media: 'video'; ms: number }

type CommandKind = 'absolute' | 'relative' | 'continuous' | 'stop' | 'center'

type MsfTrack = {
  namespace?: string
  name: string
  packaging: string
  eventType?: string
  role?: string
  isLive: boolean
  targetLatency?: number
  label?: string
  renderGroup?: number
  altGroup?: number
  initData?: string
  depends?: string[]
  temporalId?: number
  spatialId?: number
  codec?: string
  mimeType?: string
  framerate?: number
  timescale?: number
  bitrate?: number
  width?: number
  height?: number
  samplerate?: number
  channelConfig?: string
  displayWidth?: number
  displayHeight?: number
  lang?: string
  parentName?: string
  trackDuration?: number
}

type MsfCatalog = {
  version?: number
  deltaUpdate?: boolean
  addTracks?: MsfTrack[]
  removeTracks?: Array<{ namespace?: string; name: string }>
  cloneTracks?: MsfTrack[]
  generatedAt?: number
  isComplete?: boolean
  tracks?: MsfTrack[]
}

type CatalogTrack = {
  name: string
  label: string
  namespace?: string
  codec?: string
  resolution?: string
  role?: string
  packaging?: string
  isLive?: boolean
  targetLatency?: number
  framerate?: number
  bitrate?: number
}

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

let videoWorkerInitialized = false
let commandTrackAlias: bigint | null = null
let commandObjectId = 0n
let catalogTrackAlias: bigint | null = null
let catalogTracks: CatalogTrack[] = []
let videoSubscribed = false
let selectedVideoTrack: string | null = null

function setupVideoDecoderWorker(): void {
  if (videoWorkerInitialized) return

  videoWorkerInitialized = true
  const videoGenerator = new MediaStreamTrackGenerator({ kind: 'video' })
  const videoWriter = videoGenerator.writable.getWriter()
  const videoStream = new MediaStream([videoGenerator])
  const videoElement = document.getElementById('video') as HTMLVideoElement | null
  if (videoElement) {
    videoElement.srcObject = videoStream
  }

  videoDecoderWorker.onmessage = async (event: MessageEvent<VideoDecoderWorkerMessage>) => {
    if (event.data.type !== 'frame') {
      return
    }
    await videoWriter.ready
    await videoWriter.write(event.data.frame)
    event.data.frame.close()
    if (videoElement) {
      await videoElement.play()
    }
  }
}

function setupVideoCallbacks(trackAlias: bigint): void {
  setupVideoDecoderWorker()
  moqtClient.setOnSubgroupObjectHandler(trackAlias, (groupId, subgroupStreamObject) => {
    const payload = new Uint8Array(subgroupStreamObject.objectPayload)
    videoDecoderWorker.postMessage(
      {
        groupId,
        subgroupStreamObject: {
          objectId: subgroupStreamObject.objectId,
          objectPayloadLength: subgroupStreamObject.objectPayloadLength,
          objectPayload: payload,
          objectStatus: subgroupStreamObject.objectStatus,
          locHeader: subgroupStreamObject.locHeader
        }
      },
      [payload.buffer]
    )
  })
}

function updateStatus(label: string, active: boolean): void {
  const status = document.getElementById('status')
  if (!status) return
  status.innerHTML = `<span></span>${label}`
  const dot = status.querySelector('span')
  if (dot instanceof HTMLElement) {
    dot.style.background = active ? '#2ec4b6' : '#ff9f1c'
  }
}

function updateCommandAlias(alias: bigint | null): void {
  const element = document.getElementById('command-alias')
  if (!element) return
  element.textContent = alias === null ? '-' : alias.toString()
}

function updateCatalogAlias(alias: bigint | null): void {
  const element = document.getElementById('catalog-alias')
  if (!element) return
  element.textContent = alias === null ? '-' : alias.toString()
}

function updateSelectedVideoTrackLabel(track: string | null): void {
  const element = document.getElementById('selected-video-track')
  if (!element) return
  element.textContent = track ?? '-'
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

function applySelectedVideoTrack(track: string): void {
  selectedVideoTrack = track
  updateSelectedVideoTrackLabel(track)
  notifyDecoderFromCatalog(track)
}

function notifyDecoderFromCatalog(trackName: string | null): void {
  if (!trackName) return
  const track = catalogTracks.find((entry) => entry.name === trackName)
  if (!track?.codec) return
  videoDecoderWorker.postMessage({ type: 'catalog', codec: track.codec })
}

function appendMeta(container: HTMLElement, label: string, value?: string): void {
  if (!value) return
  const item = document.createElement('span')
  item.textContent = `${label}=${value}`
  container.appendChild(item)
}

function formatNumber(value?: number): string | undefined {
  if (typeof value !== 'number') return undefined
  return value.toFixed(2).replace(/\.?0+$/, '')
}

function formatBitrate(value?: number): string | undefined {
  if (typeof value !== 'number') return undefined
  if (value >= 1_000_000) {
    const mbps = (value / 1_000_000).toFixed(2).replace(/\.?0+$/, '')
    return `${mbps}Mbps`
  }
  if (value >= 1_000) {
    return `${Math.round(value / 1_000)}kbps`
  }
  return `${Math.round(value)}bps`
}

function formatLatency(value?: number): string | undefined {
  if (typeof value !== 'number') return undefined
  return `${Math.round(value)}ms`
}

function formatLive(value?: boolean): string | undefined {
  if (typeof value !== 'boolean') return undefined
  return value ? 'live' : 'ended'
}

function renderCatalogList(): void {
  const list = document.getElementById('catalog-list')
  if (!list) return
  list.innerHTML = ''
  if (catalogTracks.length === 0) {
    const empty = document.createElement('div')
    empty.className = 'muted'
    empty.textContent = 'No catalog data yet.'
    list.appendChild(empty)
    updateSelectedVideoTrackLabel(selectedVideoTrack)
    return
  }
  let currentTrack = selectedVideoTrack ?? ''
  if (currentTrack && !catalogTracks.some((track) => track.name === currentTrack)) {
    currentTrack = ''
  }
  if (!currentTrack) {
    const firstTrack = catalogTracks[0]?.name
    if (firstTrack) {
      applySelectedVideoTrack(firstTrack)
      currentTrack = firstTrack
    }
  }
  for (const [index, track] of catalogTracks.entries()) {
    const item = document.createElement('label')
    item.className = 'catalog-item'

    const header = document.createElement('div')
    header.className = 'command-title'
    header.textContent = track.label || `Track ${index + 1}`

    const subtitle = document.createElement('div')
    subtitle.className = 'catalog-subtitle'
    subtitle.textContent = track.namespace ? `${track.name} @ ${track.namespace}` : track.name

    const inputRow = document.createElement('div')
    const radio = document.createElement('input')
    radio.type = 'radio'
    radio.name = 'catalog-profile'
    radio.value = track.name
    radio.checked = currentTrack === track.name || (!currentTrack && index === 0)
    radio.addEventListener('change', () => {
      applySelectedVideoTrack(track.name)
    })
    const trackLabel = document.createElement('span')
    trackLabel.textContent = track.name
    inputRow.append(radio, trackLabel)

    const meta = document.createElement('div')
    meta.className = 'catalog-meta'
    appendMeta(meta, 'role', track.role)
    appendMeta(meta, 'packaging', track.packaging)
    appendMeta(meta, 'status', formatLive(track.isLive))
    appendMeta(meta, 'resolution', track.resolution)
    appendMeta(meta, 'codec', track.codec)
    appendMeta(meta, 'fps', formatNumber(track.framerate))
    appendMeta(meta, 'bitrate', formatBitrate(track.bitrate))
    appendMeta(meta, 'latency', formatLatency(track.targetLatency))

    item.append(header, subtitle, inputRow, meta)
    list.appendChild(item)
  }
  updateSelectedVideoTrackLabel(selectedVideoTrack)
}

function setupCatalogCallbacks(trackAlias: bigint): void {
  moqtClient.setOnSubgroupObjectHandler(trackAlias, (_groupId, subgroupStreamObject) => {
    const payloadBytes = new Uint8Array(subgroupStreamObject.objectPayload)
    const payload = new TextDecoder().decode(payloadBytes)
    try {
      const parsed = parse_msf_catalog_json(payload) as MsfCatalog
      catalogTracks = buildCatalogTracks(parsed)
      renderCatalogList()
      notifyDecoderFromCatalog(selectedVideoTrack)
      updateStatus('catalog received', true)
    } catch (err) {
      console.error(err)
      updateStatus('catalog parse failed', false)
    }
  })
}

function buildCatalogTracks(catalog: MsfCatalog): CatalogTrack[] {
  const tracks = catalog.tracks ?? catalog.addTracks ?? []
  return tracks.map((track) => {
    const label = track.label ?? track.name
    const resolution =
      typeof track.width === 'number' && typeof track.height === 'number' ? `${track.width}x${track.height}` : undefined
    return {
      name: track.name,
      label,
      namespace: track.namespace,
      codec: track.codec,
      resolution,
      role: track.role,
      packaging: track.packaging,
      isLive: track.isLive,
      targetLatency: track.targetLatency,
      framerate: track.framerate,
      bitrate: track.bitrate
    }
  })
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
  await client.sendDatagramObject(commandTrackAlias, 0n, commandObjectId, 0, bytes)
  commandObjectId += 1n
  updateStatus(`sent ${command}`, true)
}

function buildCommandUI(): void {
  const grid = document.getElementById('command-grid')
  if (!grid) return
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
  const urlInput = document.getElementById('moqt-url') as HTMLInputElement | null
  if (!urlInput) return
  const buttons = document.querySelectorAll<HTMLButtonElement>('button[data-url]')
  buttons.forEach((button) => {
    button.addEventListener('click', () => {
      const url = button.dataset.url
      if (url) {
        urlInput.value = url
      }
    })
  })
}

function openSettingsModal(): void {
  const modal = document.getElementById('settings-modal')
  if (!modal) return
  modal.classList.add('open')
  modal.setAttribute('aria-hidden', 'false')
}

function closeSettingsModal(): void {
  const modal = document.getElementById('settings-modal')
  if (!modal) return
  modal.classList.remove('open')
  modal.setAttribute('aria-hidden', 'true')
}

async function subscribeCatalog(): Promise<void> {
  ensureClient()
  const subscribeNamespace = parseNamespace((document.getElementById('subscribe-namespace') as HTMLInputElement).value)
  const catalogTrack = (document.getElementById('catalog-track') as HTMLInputElement).value
  const authInfo = (document.getElementById('auth-info') as HTMLInputElement).value
  const catalogSubscribeId = parseBigInt((document.getElementById('catalog-subscribe-id') as HTMLInputElement).value)
  const catalogAlias = parseBigInt((document.getElementById('catalog-track-alias') as HTMLInputElement).value)

  if (!subscribeNamespace.length) {
    updateStatus('namespace required', false)
    return
  }

  setupCatalogCallbacks(catalogAlias)
  await moqtClient.subscribe(catalogSubscribeId, catalogAlias, subscribeNamespace, catalogTrack, authInfo)
  catalogTrackAlias = catalogAlias
  updateCatalogAlias(catalogTrackAlias)
  updateStatus('catalog subscribed', true)
}

async function subscribeSelectedVideo(): Promise<void> {
  if (videoSubscribed) {
    updateStatus('video already subscribed', false)
    return
  }
  ensureClient()
  const subscribeNamespace = parseNamespace((document.getElementById('subscribe-namespace') as HTMLInputElement).value)
  const videoTrack = selectedVideoTrack?.trim() ?? ''
  const authInfo = (document.getElementById('auth-info') as HTMLInputElement).value
  const videoSubscribeId = parseBigInt((document.getElementById('video-subscribe-id') as HTMLInputElement).value)
  const videoTrackAlias = parseBigInt((document.getElementById('video-track-alias') as HTMLInputElement).value)

  if (!subscribeNamespace.length || !videoTrack) {
    updateStatus('video track required', false)
    return
  }

  setupVideoCallbacks(videoTrackAlias)
  await moqtClient.subscribe(videoSubscribeId, videoTrackAlias, subscribeNamespace, videoTrack, authInfo)
  videoSubscribed = true
  updateStatus(`video subscribed: ${videoTrack}`, true)
}

async function connect(): Promise<void> {
  const url = (document.getElementById('moqt-url') as HTMLInputElement).value
  const publishNamespace = parseNamespace((document.getElementById('publish-namespace') as HTMLInputElement).value)
  const subscribeNamespace = parseNamespace((document.getElementById('subscribe-namespace') as HTMLInputElement).value)
  const publishNamespaceLabel = publishNamespace.join('/')
  const commandTrack = (document.getElementById('command-track') as HTMLInputElement).value
  const authInfo = (document.getElementById('auth-info') as HTMLInputElement).value
  const maxSubscribeId = parseBigInt((document.getElementById('max-subscribe-id') as HTMLInputElement).value)

  if (!publishNamespace.length || !subscribeNamespace.length) {
    updateStatus('namespace required', false)
    return
  }

  catalogTracks = []
  videoSubscribed = false
  catalogTrackAlias = null
  selectedVideoTrack = null
  renderCatalogList()
  updateCatalogAlias(catalogTrackAlias)
  updateSelectedVideoTrackLabel(selectedVideoTrack)

  updateStatus('connecting', true)
  await moqtClient.connect(url, { sendSetup: false })
  moqtClient.setOnConnectionClosedHandler(() => {
    updateStatus('disconnected', false)
  })

  await moqtClient.sendSetupMessage(new BigUint64Array([0xff00000an]), maxSubscribeId)

  moqtClient.setOnIncomingSubscribeHandler(async ({ subscribe, isSuccess, code, respondOk, respondError }) => {
    if (!isSuccess) {
      await respondError(BigInt(code), 'subscribe rejected')
      return
    }

    if (subscribe.trackName !== commandTrack || subscribe.trackNamespace.join('/') !== publishNamespaceLabel) {
      await respondError(404n, 'unknown track')
      return
    }

    await respondOk(0n, authInfo, 'datagram')
    commandTrackAlias = subscribe.trackAlias
    updateCommandAlias(commandTrackAlias)
  })

  await moqtClient.announce(publishNamespace, authInfo)
  updateStatus('connected', true)
}

async function disconnect(): Promise<void> {
  await moqtClient.disconnect()
  commandTrackAlias = null
  commandObjectId = 0n
  catalogTrackAlias = null
  catalogTracks = []
  videoSubscribed = false
  selectedVideoTrack = null
  moqtClient.clearSubgroupObjectHandlers()
  updateCommandAlias(commandTrackAlias)
  updateCatalogAlias(catalogTrackAlias)
  renderCatalogList()
  updateSelectedVideoTrackLabel(selectedVideoTrack)
  updateStatus('disconnected', false)
}

const connectBtn = document.getElementById('connect-btn') as HTMLButtonElement | null
const disconnectBtn = document.getElementById('disconnect-btn') as HTMLButtonElement | null
const catalogSubscribeBtn = document.getElementById('catalog-subscribe-btn') as HTMLButtonElement | null
const videoSubscribeBtn = document.getElementById('video-subscribe-btn') as HTMLButtonElement | null
const settingsBtn = document.getElementById('settings-btn') as HTMLButtonElement | null
const settingsCloseBtn = document.getElementById('settings-close-btn') as HTMLButtonElement | null
const settingsSaveBtn = document.getElementById('settings-save-btn') as HTMLButtonElement | null
const settingsModal = document.getElementById('settings-modal')

connectBtn?.addEventListener('click', () => {
  connect().catch((err) => {
    console.error(err)
    updateStatus('connection failed', false)
  })
})

disconnectBtn?.addEventListener('click', () => {
  disconnect().catch((err) => {
    console.error(err)
  })
})

catalogSubscribeBtn?.addEventListener('click', () => {
  subscribeCatalog().catch((err) => {
    console.error(err)
    updateStatus('catalog subscribe failed', false)
  })
})

videoSubscribeBtn?.addEventListener('click', () => {
  subscribeSelectedVideo().catch((err) => {
    console.error(err)
    updateStatus('video subscribe failed', false)
  })
})

settingsBtn?.addEventListener('click', () => {
  openSettingsModal()
})

settingsCloseBtn?.addEventListener('click', () => {
  closeSettingsModal()
})

settingsSaveBtn?.addEventListener('click', () => {
  closeSettingsModal()
})

settingsModal?.addEventListener('click', (event) => {
  if (event.target === settingsModal) {
    closeSettingsModal()
  }
})

document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && settingsModal?.classList.contains('open')) {
    closeSettingsModal()
  }
})

buildCommandUI()
setupUrlPresets()
renderCatalogList()
