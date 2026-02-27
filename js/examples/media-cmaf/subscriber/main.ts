import { MoqtClientWrapper } from '@moqt/moqtClient'
import { parse_msf_catalog_json } from '../../../pkg/moqt_client_wasm'
import { AUTH_INFO, CMAF_FPS, CMAF_KEYFRAME_INTERVAL_FRAMES } from '../const'
import { getFormElement } from '../utils'
import { extractCatalogVideoTracks, type MediaCatalogTrack } from '../catalog'

const moqtClient = new MoqtClientWrapper()

let handlersInitialized = false
let catalogVideoTracks: MediaCatalogTrack[] = []
let selectedVideoTrackName: string | null = null

// MSE state
let mediaSource: MediaSource | null = null
let sourceBuffer: SourceBuffer | null = null
let initSegmentReceived = false
const appendQueue: Uint8Array[] = []
let isAppending = false

// Stats state
let lastNetworkDelay = 0
let lastMediaSegmentTime: number | undefined
let lastSegArrivalInterval = 0
let lastBitrate = 0
let statsIntervalId: ReturnType<typeof setInterval> | null = null
const gopSec = CMAF_KEYFRAME_INTERVAL_FRAMES / CMAF_FPS

function toBigUint64Array(value: string): BigUint64Array {
  const values = value
    .split(',')
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
    .map((part) => BigInt(part))
  return new BigUint64Array(values)
}

function parseTrackNamespace(value: string): string[] {
  return value
    .split('/')
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
}

function setCatalogTrackStatus(text: string): void {
  const status = document.getElementById('catalog-track-status')
  if (!status) {
    return
  }
  const normalized = text.trim()
  status.textContent = normalized
  status.style.display = normalized.length > 0 ? '' : 'none'
}

function getCatalogTrackSelect(): HTMLSelectElement | null {
  return document.getElementById('selected-video-track') as HTMLSelectElement | null
}

function formatCatalogTrackLabel(track: MediaCatalogTrack): string {
  const resolution =
    typeof track.width === 'number' && typeof track.height === 'number' ? ` (${track.width}x${track.height})` : ''
  return `${track.label}${resolution}`
}

function renderCatalogTrackSelect(): boolean {
  const select = getCatalogTrackSelect()
  if (!select) {
    return false
  }
  select.innerHTML = ''

  if (!catalogVideoTracks.length) {
    const option = document.createElement('option')
    option.value = ''
    option.textContent = 'Catalog video tracks are not loaded yet'
    option.disabled = true
    option.selected = true
    select.appendChild(option)
    selectedVideoTrackName = null
    return false
  }

  if (!selectedVideoTrackName || !catalogVideoTracks.some((track) => track.name === selectedVideoTrackName)) {
    selectedVideoTrackName = catalogVideoTracks[0].name
  }

  for (const track of catalogVideoTracks) {
    const option = document.createElement('option')
    option.value = track.name
    option.textContent = formatCatalogTrackLabel(track)
    option.selected = track.name === selectedVideoTrackName
    select.appendChild(option)
  }
  return true
}

function renderCatalogTracks(): void {
  const hasVideoTracks = renderCatalogTrackSelect()

  if (!hasVideoTracks) {
    setCatalogTrackStatus('')
    return
  }
  setCatalogTrackStatus(`Catalog loaded: video=${catalogVideoTracks.length}`)
}

function setupCatalogSelectionHandler(): void {
  const videoSelect = getCatalogTrackSelect()
  if (videoSelect) {
    videoSelect.addEventListener('change', () => {
      const value = videoSelect.value.trim()
      selectedVideoTrackName = value.length > 0 ? value : null
    })
  }
}

function setupCatalogCallbacks(trackAlias: bigint): void {
  moqtClient.setOnSubgroupObjectHandler(trackAlias, (_groupId, subgroupStreamObject) => {
    const payload = new TextDecoder().decode(new Uint8Array(subgroupStreamObject.objectPayload))
    try {
      const parsed = parse_msf_catalog_json(payload)
      catalogVideoTracks = extractCatalogVideoTracks(parsed)
      renderCatalogTracks()
    } catch (error) {
      console.error('[CmafSubscriber] failed to parse catalog', error)
      setCatalogTrackStatus('Catalog parse failed')
    }
  })
}

// --- MSE helpers ---

// TODO: 音声コーデックがハードコードされている。映像と音声を別 MoQ トラックに
// 分離した際に、カタログから音声コーデックを取得するようにすべき。
function buildMimeType(codec: string | undefined): string {
  return `video/mp4; codecs="${codec ?? 'avc1.640032'}, mp4a.40.2"`
}

function appendBuffer(data: Uint8Array): void {
  appendQueue.push(data)
  flushAppendQueue()
}

function flushAppendQueue(): void {
  if (isAppending || !sourceBuffer || appendQueue.length === 0) {
    return
  }
  isAppending = true
  const data = appendQueue.shift()!
  try {
    sourceBuffer.appendBuffer(data)
  } catch (e) {
    isAppending = false
    console.error('[CmafSubscriber] appendBuffer failed', e, {
      queueLength: appendQueue.length,
      buffered:
        sourceBuffer.buffered.length > 0
          ? `${sourceBuffer.buffered.start(0).toFixed(2)}-${sourceBuffer.buffered.end(0).toFixed(2)}`
          : '(empty)'
    })
  }
}

function setupMediaSource(): void {
  mediaSource = new MediaSource()
  const videoElement = document.getElementById('video') as HTMLVideoElement
  videoElement.src = URL.createObjectURL(mediaSource)

  mediaSource.addEventListener('sourceopen', () => {
    const track = catalogVideoTracks.find((t) => t.name === selectedVideoTrackName)
    const mimeType = buildMimeType(track?.codec)
    console.info('[CmafSubscriber] MediaSource open, adding SourceBuffer', { mimeType })
    sourceBuffer = mediaSource!.addSourceBuffer(mimeType)
    sourceBuffer.mode = 'sequence'
    sourceBuffer.addEventListener('updateend', () => {
      isAppending = false
      flushAppendQueue()
    })
    sourceBuffer.addEventListener('error', (e) => {
      console.error('[CmafSubscriber] SourceBuffer error', e)
    })
    flushAppendQueue()
  })
}

function setupTrackObjectCallbacks(trackAlias: bigint): void {
  initSegmentReceived = false

  moqtClient.setOnSubgroupObjectHandler(trackAlias, (_groupId, subgroupStreamObject) => {
    const raw = new Uint8Array(subgroupStreamObject.objectPayload)
    const objectId = subgroupStreamObject.objectId

    // Read 8-byte timestamp prefix and extract fMP4 payload
    const sendTime = Number(new DataView(raw.buffer, raw.byteOffset).getBigUint64(0))
    lastNetworkDelay = Date.now() - sendTime
    const payload = raw.subarray(8)

    console.info('[CmafSubscriber] recv object', {
      groupId: _groupId,
      objectId,
      byteLength: payload.byteLength,
      networkDelay: lastNetworkDelay
    })

    if (objectId === 0n) {
      if (!initSegmentReceived) {
        console.info('[CmafSubscriber] init segment received', { byteLength: payload.byteLength })
        initSegmentReceived = true
        appendBuffer(payload)
      }
      return
    }

    const now = Date.now()
    if (lastMediaSegmentTime) {
      lastSegArrivalInterval = now - lastMediaSegmentTime
      lastBitrate = payload.byteLength * 8 / (lastSegArrivalInterval / 1000) / 1000
    }
    lastMediaSegmentTime = now

    appendBuffer(payload)
  })
}

// --- Stats ---

function updateStats(): void {
  const statsEl = document.getElementById('stats')
  if (!statsEl) return

  const videoElement = document.getElementById('video') as HTMLVideoElement

  let bufferAhead = 0
  if (sourceBuffer?.buffered.length) {
    bufferAhead = sourceBuffer.buffered.end(0) - videoElement.currentTime
  }

  const quality = videoElement.getVideoPlaybackQuality?.()
  const dropped = quality?.droppedVideoFrames ?? 0
  const total = quality?.totalVideoFrames ?? 0

  const segArrival = lastSegArrivalInterval > 0 ? `${(lastSegArrivalInterval / 1000).toFixed(1)}s` : '-'
  statsEl.textContent =
    `Delay: GoP 0~${gopSec}s avg ${gopSec / 2}s + Transport ${lastNetworkDelay}ms + Buffer ${bufferAhead.toFixed(1)}s (w/o encode/decode)` +
    `\nSeg arrival: ${segArrival} | Bitrate: ${lastBitrate.toFixed(0)} kbps | Dropped: ${dropped}/${total}`
}

function startStats(): void {
  if (statsIntervalId !== null) return
  statsIntervalId = setInterval(updateStats, 1000)
}

function stopStats(): void {
  if (statsIntervalId !== null) {
    clearInterval(statsIntervalId)
    statsIntervalId = null
  }
  const statsEl = document.getElementById('stats')
  if (statsEl) statsEl.textContent = ''
}

// --- Button handlers ---

function sendSetupButtonClickHandler(): void {
  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement
  sendSetupBtn.addEventListener('click', async () => {
    const form = getFormElement()
    const versions = toBigUint64Array('0xff00000A')
    const maxSubscribeId = BigInt(form['max-subscribe-id'].value)
    await moqtClient.sendSetupMessage(versions, maxSubscribeId)
  })
}

function sendCatalogSubscribeButtonClickHandler(): void {
  const sendCatalogSubscribeBtn = document.getElementById('sendCatalogSubscribeBtn') as HTMLButtonElement
  sendCatalogSubscribeBtn.addEventListener('click', async () => {
    const form = getFormElement()
    const trackNamespace = parseTrackNamespace(form['subscribe-track-namespace'].value)
    const catalogTrackName = form['catalog-track-name'].value.trim()
    const catalogSubscribeId = BigInt(form['catalog-subscribe-id'].value)
    const catalogTrackAlias = BigInt(form['catalog-track-alias'].value)

    if (!catalogTrackName) {
      setCatalogTrackStatus('Catalog track is required')
      return
    }
    setupCatalogCallbacks(catalogTrackAlias)
    await moqtClient.subscribe(catalogSubscribeId, catalogTrackAlias, trackNamespace, catalogTrackName, AUTH_INFO)
    setCatalogTrackStatus(`Catalog subscribed: ${catalogTrackName}`)
  })
}

function sendSubscribeButtonClickHandler(): void {
  const sendSubscribeBtn = document.getElementById('sendSubscribeBtn') as HTMLButtonElement
  sendSubscribeBtn.addEventListener('click', async () => {
    const form = getFormElement()
    const trackNamespace = parseTrackNamespace(form['subscribe-track-namespace'].value)
    const selectedVideoTrack = selectedVideoTrackName ?? ''
    const videoSubscribeId = BigInt(form['video-subscribe-id'].value)
    const videoTrackAlias = BigInt(form['video-track-alias'].value)

    if (!selectedVideoTrack) {
      setCatalogTrackStatus('Select a video track from catalog first')
      return
    }

    setupMediaSource()
    setupTrackObjectCallbacks(videoTrackAlias)
    startStats()
    await moqtClient.subscribe(videoSubscribeId, videoTrackAlias, trackNamespace, selectedVideoTrack, AUTH_INFO)
    setCatalogTrackStatus(`Subscribed video=${selectedVideoTrack}`)
  })
}

function setupCloseButtonHandler(): void {
  const closeBtn = document.getElementById('closeBtn') as HTMLButtonElement
  closeBtn.addEventListener('click', async () => {
    stopStats()
    await moqtClient.disconnect()
    moqtClient.clearSubgroupObjectHandlers()
    catalogVideoTracks = []
    selectedVideoTrackName = null
    initSegmentReceived = false
    appendQueue.length = 0
    isAppending = false
    sourceBuffer = null
    if (mediaSource && mediaSource.readyState === 'open') {
      mediaSource.endOfStream()
    }
    mediaSource = null
    renderCatalogTracks()
    setCatalogTrackStatus('Disconnected')
  })
}

function setupButtonHandlers(): void {
  if (handlersInitialized) {
    return
  }
  sendSetupButtonClickHandler()
  sendCatalogSubscribeButtonClickHandler()
  sendSubscribeButtonClickHandler()
  setupCatalogSelectionHandler()
  setupCloseButtonHandler()
  handlersInitialized = true
}

// --- Client callbacks ---

moqtClient.setOnServerSetupHandler((serverSetup: any) => {
  console.log({ serverSetup })
})

moqtClient.setOnSubscribeResponseHandler((subscribeResponse) => {
  console.log({ subscribeResponse })
})

// --- Init ---

const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
connectBtn.addEventListener('click', async () => {
  const form = getFormElement()
  const url = form.url.value

  await moqtClient.connect(url, { sendSetup: false })
  setCatalogTrackStatus('Connected')
})

setupButtonHandlers()
renderCatalogTracks()
