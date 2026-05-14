import { MoqtClientWrapper } from '@moqt/moqtClient'
import { parse_msf_catalog_json } from '../../../pkg/moqt_client_wasm'
import { AUTH_INFO } from '../const'
import { getFormElement } from '../utils'
import { extractCatalogVideoTracks, extractCatalogAudioTracks, type MediaCatalogTrack } from '../catalog'

const moqtClient = new MoqtClientWrapper()

let handlersInitialized = false
let catalogVideoTracks: MediaCatalogTrack[] = []
let catalogAudioTracks: MediaCatalogTrack[] = []
let selectedVideoTrackName: string | null = null

// MSE state
let mediaSource: MediaSource | null = null

let videoSourceBuffer: SourceBuffer | null = null
let videoInitReceived = false
const videoAppendQueue: Uint8Array[] = []
let isVideoAppending = false

let audioSourceBuffer: SourceBuffer | null = null
let audioInitReceived = false
const audioAppendQueue: Uint8Array[] = []
let isAudioAppending = false

type MoqObjectVisualizationState = {
  groupId: bigint | null
  objectId: bigint | null
}

const moqObjectVisualization: Record<'video' | 'audio', MoqObjectVisualizationState> = {
  video: { groupId: null, objectId: null },
  audio: { groupId: null, objectId: null }
}

let isMoqVisualizerVisible = false

const PLAYBACK_BUFFER_THRESHOLD = 1.0 // バッファがこの秒数を超えたら再生開始（1フラグメント分）
const SOURCE_BUFFER_VISUALIZER_INTERVAL_MS = 250
const LIVE_EDGE_CATCHUP_THRESHOLD_SEC = 8
const LIVE_EDGE_TARGET_AHEAD_SEC = 1.5
const LIVE_EDGE_CATCHUP_COOLDOWN_MS = 1500
let playbackStarted = false
let sourceBufferVisualizerIntervalId: number | null = null
let lastLiveEdgeCatchupAtMs = 0
let liveEdgeCatchupHandlerInitialized = false
let isRealtimeCatchupEnabled = true

type SourceBufferSpeedState = {
  lastWallTimeMs: number | null
  lastBufferedEndSec: number | null
  speedX: number | null
}

const sourceBufferSpeedState: Record<'video' | 'audio', SourceBufferSpeedState> = {
  video: { lastWallTimeMs: null, lastBufferedEndSec: null, speedX: null },
  audio: { lastWallTimeMs: null, lastBufferedEndSec: null, speedX: null }
}

// --- Helpers ---

const toBigUint64Array = (value: string): BigUint64Array => {
  const values = value
    .split(',')
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
    .map((part) => BigInt(part))
  return new BigUint64Array(values)
}

const parseTrackNamespace = (value: string): string[] => {
  return value
    .split('/')
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
}

// --- Catalog ---

const setCatalogTrackStatus = (text: string): void => {
  const status = document.getElementById('catalog-track-status')
  if (!status) {
    return
  }
  const normalized = text.trim()
  status.textContent = normalized
  status.style.display = normalized.length > 0 ? '' : 'none'
}

const getCatalogTrackSelect = (): HTMLSelectElement | null => {
  return document.getElementById('selected-video-track') as HTMLSelectElement | null
}

const formatCatalogTrackLabel = (track: MediaCatalogTrack): string => {
  const resolution =
    typeof track.width === 'number' && typeof track.height === 'number' ? ` (${track.width}x${track.height})` : ''
  return `${track.label}${resolution}`
}

const renderCatalogTrackSelect = (): boolean => {
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

const renderCatalogTracks = (): void => {
  const hasVideoTracks = renderCatalogTrackSelect()

  if (!hasVideoTracks) {
    setCatalogTrackStatus('')
    return
  }
  setCatalogTrackStatus(`Catalog loaded: video=${catalogVideoTracks.length} audio=${catalogAudioTracks.length}`)
}

const setupCatalogSelectionHandler = (): void => {
  const videoSelect = getCatalogTrackSelect()
  if (videoSelect) {
    videoSelect.addEventListener('change', () => {
      const value = videoSelect.value.trim()
      selectedVideoTrackName = value.length > 0 ? value : null
    })
  }
}

const setupCatalogCallbacks = (trackAlias: bigint): void => {
  moqtClient.setOnSubgroupObjectHandler(trackAlias, (_groupId, subgroupStreamObject) => {
    const payload = new TextDecoder().decode(new Uint8Array(subgroupStreamObject.objectPayload))
    try {
      const parsed = parse_msf_catalog_json(payload)
      catalogVideoTracks = extractCatalogVideoTracks(parsed)
      catalogAudioTracks = extractCatalogAudioTracks(parsed)
      renderCatalogTracks()
    } catch (error) {
      console.error('[CmafSubscriber] failed to parse catalog', error)
      setCatalogTrackStatus('Catalog parse failed')
    }
  })
}

const setElementText = (id: string, text: string): void => {
  const el = document.getElementById(id)
  if (el) {
    el.textContent = text
  }
}

const getVideoElement = (): HTMLVideoElement | null => {
  return document.getElementById('video') as HTMLVideoElement | null
}

const resetSourceBufferSpeedState = (): void => {
  sourceBufferSpeedState.video = { lastWallTimeMs: null, lastBufferedEndSec: null, speedX: null }
  sourceBufferSpeedState.audio = { lastWallTimeMs: null, lastBufferedEndSec: null, speedX: null }
}

const getBufferedEnd = (sourceBuffer: SourceBuffer | null): number | null => {
  if (!sourceBuffer) {
    return null
  }
  try {
    if (sourceBuffer.buffered.length === 0) {
      return null
    }
    return sourceBuffer.buffered.end(sourceBuffer.buffered.length - 1)
  } catch {
    return null
  }
}

const getLiveEdgeTime = (): number | null => {
  const videoEnd = getBufferedEnd(videoSourceBuffer)
  const audioEnd = getBufferedEnd(audioSourceBuffer)
  if (videoEnd == null && audioEnd == null) {
    return null
  }
  if (videoEnd != null && audioEnd != null) {
    return Math.min(videoEnd, audioEnd)
  }
  return videoEnd ?? audioEnd
}

const catchupToLiveEdgeIfNeeded = (reason: 'visibility' | 'periodic'): void => {
  if (!isRealtimeCatchupEnabled) {
    return
  }
  if (!playbackStarted) {
    return
  }
  const videoElement = getVideoElement()
  if (!videoElement) {
    return
  }
  const liveEdgeTime = getLiveEdgeTime()
  if (liveEdgeTime == null || !Number.isFinite(liveEdgeTime) || !Number.isFinite(videoElement.currentTime)) {
    return
  }
  const aheadSec = liveEdgeTime - videoElement.currentTime
  if (aheadSec < LIVE_EDGE_CATCHUP_THRESHOLD_SEC) {
    return
  }
  const nowMs = performance.now()
  if (nowMs - lastLiveEdgeCatchupAtMs < LIVE_EDGE_CATCHUP_COOLDOWN_MS) {
    return
  }

  const targetTime = Math.max(0, liveEdgeTime - LIVE_EDGE_TARGET_AHEAD_SEC)
  if (!(targetTime > videoElement.currentTime)) {
    return
  }
  const from = videoElement.currentTime
  videoElement.currentTime = targetTime
  lastLiveEdgeCatchupAtMs = nowMs
  console.info('[CmafSubscriber] catch up to live edge', {
    reason,
    from: from.toFixed(2),
    target: targetTime.toFixed(2),
    liveEdge: liveEdgeTime.toFixed(2),
    ahead: aheadSec.toFixed(2)
  })
  if (videoElement.paused) {
    void videoElement.play().catch((error) => {
      console.warn('[CmafSubscriber] play failed after live-edge catchup', error)
    })
  }
}

const setupLiveEdgeCatchupHandler = (): void => {
  if (liveEdgeCatchupHandlerInitialized) {
    return
  }
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') {
      catchupToLiveEdgeIfNeeded('visibility')
    }
  })
  liveEdgeCatchupHandlerInitialized = true
}

const syncLiveCatchupModeButton = (toggleBtn: HTMLButtonElement): void => {
  toggleBtn.classList.toggle('active', isRealtimeCatchupEnabled)
  toggleBtn.setAttribute('aria-pressed', isRealtimeCatchupEnabled ? 'true' : 'false')
  if (isRealtimeCatchupEnabled) {
    toggleBtn.textContent = 'Realtime追従'
    toggleBtn.setAttribute('title', 'リアルタイム追従モード（ON）')
  } else {
    toggleBtn.textContent = '追従なし'
    toggleBtn.setAttribute('title', 'リアルタイム追従モード（OFF）')
  }
}

const setupLiveCatchupModeButton = (): void => {
  const toggleBtn = document.getElementById('toggleLiveCatchupBtn') as HTMLButtonElement | null
  if (!toggleBtn) {
    return
  }
  syncLiveCatchupModeButton(toggleBtn)

  toggleBtn.addEventListener('click', () => {
    isRealtimeCatchupEnabled = !isRealtimeCatchupEnabled
    syncLiveCatchupModeButton(toggleBtn)
    if (isRealtimeCatchupEnabled && document.visibilityState === 'visible') {
      catchupToLiveEdgeIfNeeded('periodic')
    }
  })
}

const updateSourceBufferSpeedState = (
  type: 'video' | 'audio',
  bufferedEndSec: number | null,
  wallTimeMs: number
): void => {
  const state = sourceBufferSpeedState[type]
  if (bufferedEndSec == null || !Number.isFinite(bufferedEndSec)) {
    state.lastWallTimeMs = null
    state.lastBufferedEndSec = null
    state.speedX = null
    return
  }

  if (state.lastWallTimeMs != null && state.lastBufferedEndSec != null) {
    const deltaWallSec = (wallTimeMs - state.lastWallTimeMs) / 1000
    if (deltaWallSec > 0) {
      const deltaBufferedSec = bufferedEndSec - state.lastBufferedEndSec
      const speedX = deltaBufferedSec / deltaWallSec
      state.speedX = Number.isFinite(speedX) ? speedX : null
    }
  }

  state.lastWallTimeMs = wallTimeMs
  state.lastBufferedEndSec = bufferedEndSec
}

const formatBufferedEnd = (bufferedEndSec: number | null): string => {
  if (bufferedEndSec == null) {
    return '-'
  }
  return `${bufferedEndSec.toFixed(2)}s`
}

const formatSourceBufferSpeed = (speedX: number | null): string => {
  if (speedX == null) {
    return '-'
  }
  return `${speedX.toFixed(2)}x`
}

const formatBufferedRanges = (sourceBuffer: SourceBuffer | null): string => {
  if (!sourceBuffer) {
    return '-'
  }
  try {
    if (sourceBuffer.buffered.length === 0) {
      return '-'
    }
    const ranges: string[] = []
    for (let i = 0; i < sourceBuffer.buffered.length; i += 1) {
      const start = sourceBuffer.buffered.start(i)
      const end = sourceBuffer.buffered.end(i)
      ranges.push(`${start.toFixed(2)}-${end.toFixed(2)}s`)
    }
    return ranges.join(', ')
  } catch {
    return 'unavailable'
  }
}

const formatBufferedAhead = (sourceBuffer: SourceBuffer | null, currentTime: number): string => {
  if (!sourceBuffer) {
    return '-'
  }
  try {
    if (sourceBuffer.buffered.length === 0) {
      return '0.00s'
    }
    for (let i = 0; i < sourceBuffer.buffered.length; i += 1) {
      const start = sourceBuffer.buffered.start(i)
      const end = sourceBuffer.buffered.end(i)
      if (currentTime >= start && currentTime <= end) {
        return `${(end - currentTime).toFixed(2)}s`
      }
      if (currentTime < start) {
        return '0.00s'
      }
    }
    return '0.00s'
  } catch {
    return 'unavailable'
  }
}

const renderSourceBufferVisualization = (): void => {
  const videoElement = getVideoElement()
  const wallTimeMs = performance.now()
  const currentTime = videoElement ? videoElement.currentTime : 0
  const timeText = Number.isFinite(currentTime) ? `${currentTime.toFixed(2)}s` : '-'
  const videoBufferedEnd = getBufferedEnd(videoSourceBuffer)
  const audioBufferedEnd = getBufferedEnd(audioSourceBuffer)

  updateSourceBufferSpeedState('video', videoBufferedEnd, wallTimeMs)
  updateSourceBufferSpeedState('audio', audioBufferedEnd, wallTimeMs)

  setElementText('sb-video-updating', videoSourceBuffer ? (videoSourceBuffer.updating ? 'true' : 'false') : '-')
  setElementText('sb-audio-updating', audioSourceBuffer ? (audioSourceBuffer.updating ? 'true' : 'false') : '-')
  setElementText('sb-video-queue', `${videoAppendQueue.length}`)
  setElementText('sb-audio-queue', `${audioAppendQueue.length}`)
  setElementText('sb-video-ranges', formatBufferedRanges(videoSourceBuffer))
  setElementText('sb-audio-ranges', formatBufferedRanges(audioSourceBuffer))
  setElementText('sb-video-ahead', formatBufferedAhead(videoSourceBuffer, currentTime))
  setElementText('sb-audio-ahead', formatBufferedAhead(audioSourceBuffer, currentTime))
  setElementText('sb-video-end', formatBufferedEnd(videoBufferedEnd))
  setElementText('sb-audio-end', formatBufferedEnd(audioBufferedEnd))
  setElementText('sb-video-speed', formatSourceBufferSpeed(sourceBufferSpeedState.video.speedX))
  setElementText('sb-audio-speed', formatSourceBufferSpeed(sourceBufferSpeedState.audio.speedX))
  setElementText('sb-video-time', timeText)
  setElementText('sb-audio-time', timeText)

  if (document.visibilityState === 'visible') {
    catchupToLiveEdgeIfNeeded('periodic')
  }
}

const stopSourceBufferVisualizationLoop = (): void => {
  if (sourceBufferVisualizerIntervalId != null) {
    window.clearInterval(sourceBufferVisualizerIntervalId)
    sourceBufferVisualizerIntervalId = null
  }
}

const startSourceBufferVisualizationLoop = (): void => {
  stopSourceBufferVisualizationLoop()
  resetSourceBufferSpeedState()
  lastLiveEdgeCatchupAtMs = 0
  renderSourceBufferVisualization()
  sourceBufferVisualizerIntervalId = window.setInterval(() => {
    renderSourceBufferVisualization()
  }, SOURCE_BUFFER_VISUALIZER_INTERVAL_MS)
}

const resetSourceBufferVisualization = (): void => {
  const ids = [
    'sb-video-updating',
    'sb-audio-updating',
    'sb-video-queue',
    'sb-audio-queue',
    'sb-video-ranges',
    'sb-audio-ranges',
    'sb-video-ahead',
    'sb-audio-ahead',
    'sb-video-end',
    'sb-audio-end',
    'sb-video-speed',
    'sb-audio-speed',
    'sb-video-time',
    'sb-audio-time'
  ]
  for (const id of ids) {
    setElementText(id, '-')
  }
  resetSourceBufferSpeedState()
  lastLiveEdgeCatchupAtMs = 0
}

// --- Overhead display ---

const updateOverheadDisplay = (
  type: 'video' | 'audio',
  init: number,
  moof: number,
  mdat: number,
  overhead: string
): void => {
  const setText = (id: string, text: string) => {
    const el = document.getElementById(id)
    if (el) {
      el.textContent = text
    }
  }
  setText(`oh-${type}-init`, `${init} bytes`)
  setText(`oh-${type}-moof`, `${moof} bytes`)
  setText(`oh-${type}-mdat`, `${mdat} bytes`)
  setText(`oh-${type}-pct`, `${overhead}%`)
}

const setMoqObjectVisualizationState = (type: 'video' | 'audio', groupId: bigint, objectId: bigint): void => {
  moqObjectVisualization[type] = { groupId, objectId }
  if (isMoqVisualizerVisible) {
    renderMoqObjectVisualization()
  }
}

const getVisualizerEyeSvg = (active: boolean): string => {
  if (active) {
    return `
      <svg viewBox="0 0 24 24" aria-hidden="true">
        <path d="M3 3l18 18"></path>
        <path d="M10.6 10.6A2 2 0 0 0 12 14a2 2 0 0 0 1.4-.6"></path>
        <path d="M9.9 5.3A11.6 11.6 0 0 1 12 5c6.5 0 10 7 10 7a15 15 0 0 1-4 4.9"></path>
        <path d="M6.6 6.6A15.1 15.1 0 0 0 2 12s3.5 7 10 7a11.3 11.3 0 0 0 5.1-1.2"></path>
      </svg>
    `
  }
  return `
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6-10-6-10-6z"></path>
      <circle cx="12" cy="12" r="3"></circle>
    </svg>
  `
}

const syncMoqVisualizerButton = (toggleBtn: HTMLButtonElement): void => {
  toggleBtn.innerHTML = getVisualizerEyeSvg(isMoqVisualizerVisible)
  toggleBtn.classList.toggle('active', isMoqVisualizerVisible)
  toggleBtn.setAttribute('aria-pressed', isMoqVisualizerVisible ? 'true' : 'false')
  const label = isMoqVisualizerVisible ? 'MoQ受信可視化を非表示' : 'MoQ受信可視化を表示'
  toggleBtn.setAttribute('aria-label', label)
  toggleBtn.setAttribute('title', label)
}

const resetMoqObjectVisualizationState = (): void => {
  moqObjectVisualization.video = { groupId: null, objectId: null }
  moqObjectVisualization.audio = { groupId: null, objectId: null }
}

const renderMoqObjectVisualization = (): void => {
  const setText = (id: string, text: string) => {
    const el = document.getElementById(id)
    if (el) {
      el.textContent = text
    }
  }
  const formatBigInt = (value: bigint | null): string => (value === null ? '-' : value.toString())

  setText('viz-video-group-id', formatBigInt(moqObjectVisualization.video.groupId))
  setText('viz-video-object-id', formatBigInt(moqObjectVisualization.video.objectId))
  setText('viz-audio-group-id', formatBigInt(moqObjectVisualization.audio.groupId))
  setText('viz-audio-object-id', formatBigInt(moqObjectVisualization.audio.objectId))
}

const setupMoqObjectVisualizationButton = (): void => {
  const toggleBtn = document.getElementById('toggleMoqVisualizerBtn') as HTMLButtonElement | null
  const panel = document.getElementById('moqVisualizerPanel')
  if (!toggleBtn || !panel) {
    return
  }
  syncMoqVisualizerButton(toggleBtn)

  toggleBtn.addEventListener('click', () => {
    isMoqVisualizerVisible = !isMoqVisualizerVisible
    panel.classList.toggle('is-visible', isMoqVisualizerVisible)
    syncMoqVisualizerButton(toggleBtn)
    if (isMoqVisualizerVisible) {
      renderMoqObjectVisualization()
      renderSourceBufferVisualization()
    }
  })
}

// --- MSE helpers ---

const buildVideoMimeType = (codec: string): string => {
  return `video/mp4; codecs="${codec}"`
}

const buildAudioMimeType = (codec: string): string => {
  return `audio/mp4; codecs="${codec}"`
}

const cloneBytes = (data: Uint8Array): Uint8Array<ArrayBuffer> => {
  const cloned = new Uint8Array<ArrayBuffer>(new ArrayBuffer(data.byteLength))
  cloned.set(data)
  return cloned
}

const appendToVideo = (data: Uint8Array): void => {
  videoAppendQueue.push(data)
  flushVideoQueue()
}

const flushVideoQueue = (): void => {
  if (isVideoAppending || !videoSourceBuffer || videoAppendQueue.length === 0) {
    return
  }
  isVideoAppending = true
  const data = videoAppendQueue.shift()!
  try {
    videoSourceBuffer.appendBuffer(cloneBytes(data))
  } catch (e) {
    isVideoAppending = false
    console.error('[CmafSubscriber] video appendBuffer failed', e)
  }
}

const appendToAudio = (data: Uint8Array): void => {
  audioAppendQueue.push(data)
  flushAudioQueue()
}

const flushAudioQueue = (): void => {
  if (isAudioAppending || !audioSourceBuffer || audioAppendQueue.length === 0) {
    return
  }
  isAudioAppending = true
  const data = audioAppendQueue.shift()!
  try {
    audioSourceBuffer.appendBuffer(cloneBytes(data))
  } catch (e) {
    isAudioAppending = false
    console.error('[CmafSubscriber] audio appendBuffer failed', e)
  }
}

const setupMediaSource = (videoCodec: string, audioCodec: string): void => {
  mediaSource = new MediaSource()
  const videoElement = getVideoElement()
  if (!videoElement) {
    return
  }
  videoElement.src = URL.createObjectURL(mediaSource)

  mediaSource.addEventListener('sourceopen', () => {
    const videoMime = buildVideoMimeType(videoCodec)
    const audioMime = buildAudioMimeType(audioCodec)
    console.info('[CmafSubscriber] MediaSource open, adding SourceBuffers', { videoMime, audioMime })

    videoSourceBuffer = mediaSource!.addSourceBuffer(videoMime)
    videoSourceBuffer.mode = 'sequence'
    videoSourceBuffer.addEventListener('updateend', () => {
      isVideoAppending = false
      if (!playbackStarted && videoSourceBuffer!.buffered.length > 0) {
        const buffered = videoSourceBuffer!.buffered.end(0) - videoSourceBuffer!.buffered.start(0)
        if (buffered >= PLAYBACK_BUFFER_THRESHOLD) {
          playbackStarted = true
          void videoElement.play().catch((error) => {
            console.warn('[CmafSubscriber] initial play failed', error)
          })
          console.info('[CmafSubscriber] playback started', { buffered: buffered.toFixed(2) })
        }
      }
      flushVideoQueue()
      renderSourceBufferVisualization()
    })
    videoSourceBuffer.addEventListener('error', (e) => {
      console.error('[CmafSubscriber] video SourceBuffer error', e)
    })

    audioSourceBuffer = mediaSource!.addSourceBuffer(audioMime)
    audioSourceBuffer.mode = 'sequence'
    audioSourceBuffer.addEventListener('updateend', () => {
      isAudioAppending = false
      flushAudioQueue()
      renderSourceBufferVisualization()
    })
    audioSourceBuffer.addEventListener('error', (e) => {
      console.error('[CmafSubscriber] audio SourceBuffer error', e)
    })

    flushVideoQueue()
    flushAudioQueue()
    startSourceBufferVisualizationLoop()
  })
}

// --- MoQ Object handlers ---

const setupVideoObjectCallbacks = (trackAlias: bigint): void => {
  videoInitReceived = false
  let lastVideoGroupId: bigint | null = null
  let videoInitSize = 0

  moqtClient.setOnSubgroupObjectHandler(trackAlias, (_groupId, subgroupStreamObject) => {
    const raw = new Uint8Array(subgroupStreamObject.objectPayload)
    const objectId = subgroupStreamObject.objectId
    const objectStatus = subgroupStreamObject.objectStatus
    const payload = raw

    if (_groupId !== lastVideoGroupId) {
      lastVideoGroupId = _groupId
    }

    if (objectId === 0n) {
      videoInitSize = payload.byteLength
      setMoqObjectVisualizationState('video', _groupId, objectId)
      if (!videoInitReceived) {
        console.info('[CmafSubscriber] video init segment received', { byteLength: payload.byteLength })
        videoInitReceived = true
        appendToVideo(payload)
      }
      return
    }

    if (objectStatus != null) {
      setMoqObjectVisualizationState('video', _groupId, objectId)
      return
    }

    if (payload.byteLength < 4) {
      console.warn('[CmafSubscriber] video payload too short for moof', { groupId: _groupId, objectId, objectStatus })
      return
    }

    const moofSize = new DataView(payload.buffer, payload.byteOffset, 4).getUint32(0)
    const mdatSize = payload.byteLength - moofSize
    const headerBytes = videoInitSize + moofSize
    const overhead = ((headerBytes / (headerBytes + mdatSize)) * 100).toFixed(2)
    updateOverheadDisplay('video', videoInitSize, moofSize, mdatSize, overhead)
    setMoqObjectVisualizationState('video', _groupId, objectId)
    appendToVideo(payload)
  })
}

const setupAudioObjectCallbacks = (trackAlias: bigint): void => {
  audioInitReceived = false
  let lastAudioGroupId: bigint | null = null
  let audioInitSize = 0

  moqtClient.setOnSubgroupObjectHandler(trackAlias, (_groupId, subgroupStreamObject) => {
    const raw = new Uint8Array(subgroupStreamObject.objectPayload)
    const objectId = subgroupStreamObject.objectId
    const objectStatus = subgroupStreamObject.objectStatus
    const payload = raw

    if (_groupId !== lastAudioGroupId) {
      lastAudioGroupId = _groupId
    }

    if (objectId === 0n) {
      audioInitSize = payload.byteLength
      setMoqObjectVisualizationState('audio', _groupId, objectId)
      if (!audioInitReceived) {
        console.info('[CmafSubscriber] audio init segment received', { byteLength: payload.byteLength })
        audioInitReceived = true
        appendToAudio(payload)
      }
      return
    }

    if (objectStatus != null) {
      setMoqObjectVisualizationState('audio', _groupId, objectId)
      return
    }

    if (payload.byteLength < 4) {
      console.warn('[CmafSubscriber] audio payload too short for moof', { groupId: _groupId, objectId, objectStatus })
      return
    }

    const moofSize = new DataView(payload.buffer, payload.byteOffset, 4).getUint32(0)
    const mdatSize = payload.byteLength - moofSize
    const headerBytes = audioInitSize + moofSize
    const overhead = ((headerBytes / (headerBytes + mdatSize)) * 100).toFixed(2)
    updateOverheadDisplay('audio', audioInitSize, moofSize, mdatSize, overhead)
    setMoqObjectVisualizationState('audio', _groupId, objectId)
    appendToAudio(payload)
  })
}

// --- Button handlers ---

const sendSetupButtonClickHandler = (): void => {
  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement
  sendSetupBtn.addEventListener('click', async () => {
    const form = getFormElement()
    const versions = toBigUint64Array('0xff00000E')
    const maxSubscribeId = BigInt(form['max-subscribe-id'].value)
    await moqtClient.sendClientSetup(versions, maxSubscribeId)
  })
}

const sendCatalogSubscribeButtonClickHandler = (): void => {
  const sendCatalogSubscribeBtn = document.getElementById('sendCatalogSubscribeBtn') as HTMLButtonElement
  sendCatalogSubscribeBtn.addEventListener('click', async () => {
    const form = getFormElement()
    const trackNamespace = parseTrackNamespace(form['subscribe-track-namespace'].value)
    const catalogTrackName = form['catalog-track-name'].value.trim()
    const catalogSubscribeId = BigInt(form['catalog-subscribe-id'].value)
    if (!catalogTrackName) {
      setCatalogTrackStatus('Catalog track is required')
      return
    }
    const catalogTrackAlias = await moqtClient.subscribe(
      catalogSubscribeId,
      trackNamespace,
      catalogTrackName,
      AUTH_INFO
    )
    form['catalog-track-alias'].value = catalogTrackAlias.toString()
    setupCatalogCallbacks(catalogTrackAlias)
    setCatalogTrackStatus(`Catalog subscribed: ${catalogTrackName}`)
  })
}

const sendSubscribeButtonClickHandler = (): void => {
  const sendSubscribeBtn = document.getElementById('sendSubscribeBtn') as HTMLButtonElement
  sendSubscribeBtn.addEventListener('click', async () => {
    const form = getFormElement()
    const trackNamespace = parseTrackNamespace(form['subscribe-track-namespace'].value)
    const selectedVideoTrack = selectedVideoTrackName ?? ''
    const videoSubscribeId = BigInt(form['video-subscribe-id'].value)
    const audioSubscribeId = BigInt(form['audio-subscribe-id'].value)

    if (!selectedVideoTrack) {
      setCatalogTrackStatus('Select a video track from catalog first')
      return
    }

    const videoTrack = catalogVideoTracks.find((t) => t.name === selectedVideoTrack)
    const audioTrack = catalogAudioTracks[0]

    if (!videoTrack?.codec || !audioTrack?.codec) {
      setCatalogTrackStatus('Video or audio codec not found in catalog')
      return
    }

    setupMediaSource(videoTrack.codec, audioTrack.codec)
    const videoTrackAlias = await moqtClient.subscribe(videoSubscribeId, trackNamespace, selectedVideoTrack, AUTH_INFO)
    form['video-track-alias'].value = videoTrackAlias.toString()
    setupVideoObjectCallbacks(videoTrackAlias)

    const audioTrackAlias = await moqtClient.subscribe(audioSubscribeId, trackNamespace, audioTrack.name, AUTH_INFO)
    form['audio-track-alias'].value = audioTrackAlias.toString()
    setupAudioObjectCallbacks(audioTrackAlias)
    setCatalogTrackStatus(`Subscribed video=${selectedVideoTrack} audio=${audioTrack.name}`)
  })
}

const setupCloseButtonHandler = (): void => {
  const closeBtn = document.getElementById('closeBtn') as HTMLButtonElement
  closeBtn.addEventListener('click', async () => {
    await moqtClient.disconnect()
    moqtClient.clearSubgroupObjectHandlers()
    catalogVideoTracks = []
    catalogAudioTracks = []
    selectedVideoTrackName = null
    playbackStarted = false
    lastLiveEdgeCatchupAtMs = 0
    videoInitReceived = false
    videoAppendQueue.length = 0
    isVideoAppending = false
    videoSourceBuffer = null
    audioInitReceived = false
    audioAppendQueue.length = 0
    isAudioAppending = false
    audioSourceBuffer = null
    if (mediaSource && mediaSource.readyState === 'open') {
      mediaSource.endOfStream()
    }
    mediaSource = null
    stopSourceBufferVisualizationLoop()
    resetSourceBufferVisualization()
    resetMoqObjectVisualizationState()
    if (isMoqVisualizerVisible) {
      renderMoqObjectVisualization()
    }
    renderCatalogTracks()
    setCatalogTrackStatus('Disconnected')
  })
}

const setupButtonHandlers = (): void => {
  if (handlersInitialized) {
    return
  }
  sendSetupButtonClickHandler()
  sendCatalogSubscribeButtonClickHandler()
  sendSubscribeButtonClickHandler()
  setupMoqObjectVisualizationButton()
  setupLiveCatchupModeButton()
  setupCatalogSelectionHandler()
  setupCloseButtonHandler()
  setupLiveEdgeCatchupHandler()
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
