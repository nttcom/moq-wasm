import { MoqtClientWrapper } from '@moqt/moqtClient'
import { element, parseTrackNamespace, setStatusText, initializeMediaExamplePage } from '../../../utils/page'
import { setUpCatalogPicker } from '../../media/catalogPicker'
import { setUpVisualizerToggle } from '../../media/visualizerToggle'

const moqtClient = new MoqtClientWrapper()
const form = element<HTMLFormElement>('form')

const catalogPicker = setUpCatalogPicker(moqtClient, form)

// MSE state
let mediaSource: MediaSource | null = null

let videoSourceBuffer: SourceBuffer | null = null
const videoAppendQueue: Uint8Array[] = []
let isVideoAppending = false

let audioSourceBuffer: SourceBuffer | null = null
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

const PLAYBACK_BUFFER_THRESHOLD = 1.0 // バッファがこの秒数を超えたら再生開始（1フラグメント分）
const SOURCE_BUFFER_VISUALIZER_INTERVAL_MS = 250
const LIVE_EDGE_CATCHUP_THRESHOLD_SEC = 8
const LIVE_EDGE_TARGET_AHEAD_SEC = 1.5
const LIVE_EDGE_CATCHUP_COOLDOWN_MS = 1500
let playbackStarted = false
let sourceBufferVisualizerIntervalId: number | null = null
let lastLiveEdgeCatchupAtMs = 0
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
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') {
      catchupToLiveEdgeIfNeeded('visibility')
    }
  })
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
  document.querySelectorAll('[id^="sb-"]').forEach((cell) => {
    cell.textContent = '-'
  })
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
  setElementText(`oh-${type}-init`, `${init} bytes`)
  setElementText(`oh-${type}-moof`, `${moof} bytes`)
  setElementText(`oh-${type}-mdat`, `${mdat} bytes`)
  setElementText(`oh-${type}-pct`, `${overhead}%`)
}

const setMoqObjectVisualizationState = (type: 'video' | 'audio', groupId: bigint, objectId: bigint): void => {
  moqObjectVisualization[type] = { groupId, objectId }
  if (isMoqVisualizerVisible()) {
    renderMoqObjectVisualization()
  }
}

const resetMoqObjectVisualizationState = (): void => {
  moqObjectVisualization.video = { groupId: null, objectId: null }
  moqObjectVisualization.audio = { groupId: null, objectId: null }
}

const renderMoqObjectVisualization = (): void => {
  for (const kind of ['video', 'audio'] as const) {
    const { groupId, objectId } = moqObjectVisualization[kind]
    setElementText(`viz-${kind}-group-id`, groupId?.toString() ?? '-')
    setElementText(`viz-${kind}-object-id`, objectId?.toString() ?? '-')
  }
}

const isMoqVisualizerVisible = setUpVisualizerToggle(
  'toggleMoqVisualizerBtn',
  'moqVisualizerPanel',
  'MoQ受信可視化',
  () => {
    renderMoqObjectVisualization()
    renderSourceBufferVisualization()
  }
)

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

const setupObjectCallbacks = (
  kind: 'video' | 'audio',
  trackAlias: bigint,
  append: (data: Uint8Array) => void
): void => {
  let initReceived = false
  let initSize = 0

  moqtClient.setOnSubgroupObjectHandler(trackAlias, (groupId, subgroupStreamObject) => {
    const payload = new Uint8Array(subgroupStreamObject.objectPayload)
    const { objectId, objectStatus } = subgroupStreamObject

    if (objectId === 0n) {
      initSize = payload.byteLength
      setMoqObjectVisualizationState(kind, groupId, objectId)
      if (!initReceived) {
        console.info(`[CmafSubscriber] ${kind} init segment received`, { byteLength: payload.byteLength })
        initReceived = true
        append(payload)
      }
      return
    }

    if (objectStatus != null) {
      setMoqObjectVisualizationState(kind, groupId, objectId)
      return
    }

    if (payload.byteLength < 4) {
      console.warn(`[CmafSubscriber] ${kind} payload too short for moof`, { groupId, objectId, objectStatus })
      return
    }

    const moofSize = new DataView(payload.buffer, payload.byteOffset, 4).getUint32(0)
    const mdatSize = payload.byteLength - moofSize
    const headerBytes = initSize + moofSize
    const overhead = ((headerBytes / (headerBytes + mdatSize)) * 100).toFixed(2)
    updateOverheadDisplay(kind, initSize, moofSize, mdatSize, overhead)
    setMoqObjectVisualizationState(kind, groupId, objectId)
    append(payload)
  })
}

// --- Button handlers ---

const sendSetupButtonClickHandler = (): void => {
  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement
  sendSetupBtn.addEventListener('click', async () => {
    const versions = new BigUint64Array([0xff00000en])
    const maxSubscribeId = BigInt(form['max-subscribe-id'].value)
    await moqtClient.sendClientSetup(versions, maxSubscribeId)
  })
}

const sendSubscribeButtonClickHandler = (): void => {
  const sendSubscribeBtn = document.getElementById('sendSubscribeBtn') as HTMLButtonElement
  sendSubscribeBtn.addEventListener('click', async () => {
    const trackNamespace = parseTrackNamespace(form['subscribe-track-namespace'].value)
    const videoTrack = catalogPicker.pick.video.selected
    const audioTrack = catalogPicker.pick.audio.selected
    const videoSubscribeId = BigInt(form['video-subscribe-id'].value)
    const audioSubscribeId = BigInt(form['audio-subscribe-id'].value)

    if (!videoTrack) {
      setStatusText('catalog-track-status', 'Select a video track from catalog first')
      return
    }
    if (!videoTrack.codec || !audioTrack?.codec) {
      setStatusText('catalog-track-status', 'Video or audio codec not found in catalog')
      return
    }

    setupMediaSource(videoTrack.codec, audioTrack.codec)
    const videoTrackAlias = (
      await moqtClient.subscribe(trackNamespace, videoTrack.name, 'secret', { requestId: videoSubscribeId })
    ).subscribeOk.trackAlias
    form['video-track-alias'].value = videoTrackAlias.toString()
    setupObjectCallbacks('video', videoTrackAlias, appendToVideo)

    const audioTrackAlias = (
      await moqtClient.subscribe(trackNamespace, audioTrack.name, 'secret', { requestId: audioSubscribeId })
    ).subscribeOk.trackAlias
    form['audio-track-alias'].value = audioTrackAlias.toString()
    setupObjectCallbacks('audio', audioTrackAlias, appendToAudio)
    setStatusText('catalog-track-status', `Subscribed video=${videoTrack.name} audio=${audioTrack.name}`)
  })
}

const setupCloseButtonHandler = (): void => {
  const closeBtn = document.getElementById('closeBtn') as HTMLButtonElement
  closeBtn.addEventListener('click', async () => {
    await moqtClient.disconnect()
    moqtClient.clearSubgroupObjectHandlers()
    playbackStarted = false
    lastLiveEdgeCatchupAtMs = 0
    videoAppendQueue.length = 0
    isVideoAppending = false
    videoSourceBuffer = null
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
    if (isMoqVisualizerVisible()) {
      renderMoqObjectVisualization()
    }
    catalogPicker.reset()
    setStatusText('catalog-track-status', 'Disconnected')
  })
}

const setupButtonHandlers = (): void => {
  sendSetupButtonClickHandler()
  sendSubscribeButtonClickHandler()
  setupLiveCatchupModeButton()
  setupCloseButtonHandler()
  setupLiveEdgeCatchupHandler()
}

// --- Client callbacks ---

moqtClient.setOnServerSetupHandler((serverSetup: any) => {
  console.log({ serverSetup })
})

moqtClient.setOnSubscribeResponseHandler((subscribeResponse) => {
  console.log({ subscribeResponse })
})

// --- Init ---

initializeMediaExamplePage('subscribe-track-namespace')

const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
connectBtn.addEventListener('click', async () => {
  const url = form.url.value

  await moqtClient.connect(url, { sendSetup: false })
  setStatusText('catalog-track-status', 'Connected')
})

setupButtonHandlers()
