import { MoqtClientWrapper } from '@moqt/moqtClient'
import { parse_msf_catalog_json } from '../../../pkg/moqt_client_wasm'
import { AUTH_INFO } from './const'
import { getFormElement } from './utils'
import { summarizeLocHeader } from '../../../utils/media/locSummary'
import {
  extractCatalogAudioTracks,
  extractCatalogVideoTracks,
  getResolvedMediaVideoCodec,
  type MediaCatalogTrack
} from '../catalog'
import { initializeMediaExamplePage, parseTrackNamespace, setStatusText } from '../common'

const moqtClient = new MoqtClientWrapper()

const audioDecoderWorker = new Worker(new URL('../../../utils/media/decoders/audioDecoder.ts', import.meta.url), {
  type: 'module'
})
const videoDecoderWorker = new Worker(new URL('../../../utils/media/decoders/videoDecoder.ts', import.meta.url), {
  type: 'module'
})

type AudioDecoderWorkerMessage =
  | { type: 'audioData'; audioData: AudioData }
  | { type: 'bitrate'; media: 'audio'; kbps: number }
  | { type: 'receiveLatency'; media: 'audio'; ms: number }
  | { type: 'renderingLatency'; media: 'audio'; ms: number }
  | { type: 'bufferedObject'; media: 'audio'; groupId: bigint; objectId: bigint }

type VideoDecoderWorkerMessage =
  | { type: 'frame'; frame: VideoFrame }
  | { type: 'bitrate'; kbps: number }
  | { type: 'receiveLatency'; media: 'video'; ms: number }
  | { type: 'renderingLatency'; media: 'video'; ms: number }
  | { type: 'bufferedObject'; media: 'video'; groupId: bigint; objectId: bigint }

let audioWorkerInitialized = false
let videoWorkerInitialized = false
let handlersInitialized = false
let catalogVideoTracks: MediaCatalogTrack[] = []
let catalogAudioTracks: MediaCatalogTrack[] = []
let selectedVideoTrackName: string | null = null
let selectedAudioTrackName: string | null = null
let audioPlaybackStarted = false
let videoPlaybackStarted = false
let receivedVideoObjectCount = 0
let receivedAudioObjectCount = 0

function shouldBypassJitterBuffer(): boolean {
  const input = document.getElementById('bypass-jitter-buffer') as HTMLInputElement | null
  return input?.checked ?? true
}

function applyDecoderWorkerConfig(): void {
  const bypassJitterBuffer = shouldBypassJitterBuffer()
  const config = { telemetryEnabled: true, bypassJitterBuffer }
  audioDecoderWorker.postMessage({ type: 'config', config })
  videoDecoderWorker.postMessage({ type: 'config', config })
}

function setPlaybackObjectStatus(kind: 'video' | 'audio', text: string): void {
  const element = document.getElementById(`${kind}-playback-object`)
  if (!element) {
    return
  }
  element.textContent = text
}

function setPlaybackObjectPosition(kind: 'video' | 'audio', groupId: bigint, objectId: bigint): void {
  setPlaybackObjectStatus(kind, `groupId=${groupId.toString()} objectId=${objectId.toString()}`)
}

function toBigUint64Array(value: string): BigUint64Array {
  const values = value
    .split(',')
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
    .map((part) => BigInt(part))
  return new BigUint64Array(values)
}

function setCatalogTrackStatus(text: string): void {
  setStatusText('catalog-track-status', text)
}

function setConnectionStatus(text: string): void {
  setStatusText('subscriber-connection-status', text)
}

function setSetupStatus(text: string): void {
  setStatusText('subscriber-setup-status', text)
}

function setTrackSubscribeStatus(text: string): void {
  setStatusText('subscriber-track-status', text)
}

function setReceiveStatus(text: string): void {
  setStatusText('subscriber-receive-status', text)
}

function setPlaybackStatus(text: string): void {
  setStatusText('subscriber-playback-status', text)
}

function initializeStatuses(): void {
  setConnectionStatus('Not connected')
  setSetupStatus('Setup not sent')
  setCatalogTrackStatus('Catalog not loaded yet')
  setTrackSubscribeStatus('Subscription idle')
  setReceiveStatus('Waiting for media objects')
  setPlaybackStatus('Playback idle')
}

function getCatalogTrackSelect(kind: 'video' | 'audio'): HTMLSelectElement | null {
  const id = kind === 'video' ? 'selected-video-track' : 'selected-audio-track'
  return document.getElementById(id) as HTMLSelectElement | null
}

function getCatalogTracks(kind: 'video' | 'audio'): MediaCatalogTrack[] {
  return kind === 'video' ? catalogVideoTracks : catalogAudioTracks
}

function getSelectedCatalogTrackName(kind: 'video' | 'audio'): string | null {
  return kind === 'video' ? selectedVideoTrackName : selectedAudioTrackName
}

function setSelectedCatalogTrackName(kind: 'video' | 'audio', trackName: string | null): void {
  if (kind === 'video') {
    selectedVideoTrackName = trackName
  } else {
    selectedAudioTrackName = trackName
  }
}

function setSelectedCatalogTrack(kind: 'video' | 'audio', trackName: string | null): void {
  setSelectedCatalogTrackName(kind, trackName)
  const select = getCatalogTrackSelect(kind)
  if (select && trackName) {
    select.value = trackName
  }
  if (kind !== 'video') {
    return
  }
  const track = catalogVideoTracks.find((entry) => entry.name === trackName)
  if (trackName) {
    videoDecoderWorker.postMessage({ type: 'catalog', codec: track?.codec ?? getResolvedMediaVideoCodec() })
  }
}

function formatCatalogTrackLabel(track: MediaCatalogTrack, kind: 'video' | 'audio'): string {
  if (kind === 'video') {
    const resolution =
      typeof track.width === 'number' && typeof track.height === 'number' ? ` (${track.width}x${track.height})` : ''
    return `${track.label}${resolution}`
  }
  const details: string[] = []
  if (track.codec) {
    details.push(track.codec)
  }
  if (typeof track.samplerate === 'number') {
    details.push(`${track.samplerate}Hz`)
  }
  if (track.channelConfig) {
    details.push(track.channelConfig)
  }
  if (typeof track.bitrate === 'number') {
    details.push(`${Math.round(track.bitrate / 1000)}kbps`)
  }
  const metadata = details.length > 0 ? ` (${details.join(', ')})` : ''
  return `${track.label}${metadata}`
}

function renderCatalogTrackSelect(kind: 'video' | 'audio'): boolean {
  const select = getCatalogTrackSelect(kind)
  if (!select) {
    return false
  }
  select.innerHTML = ''
  const tracks = getCatalogTracks(kind)
  const isVideo = kind === 'video'
  const emptyMessage = isVideo ? 'Catalog video tracks are not loaded yet' : 'Catalog audio tracks are not loaded yet'

  if (!tracks.length) {
    const option = document.createElement('option')
    option.value = ''
    option.textContent = emptyMessage
    option.disabled = true
    option.selected = true
    select.appendChild(option)
    setSelectedCatalogTrack(kind, null)
    return false
  }

  let selectedTrackName = getSelectedCatalogTrackName(kind)
  if (!selectedTrackName || !tracks.some((track) => track.name === selectedTrackName)) {
    selectedTrackName = tracks[0].name
    setSelectedCatalogTrackName(kind, selectedTrackName)
  }

  for (const track of tracks) {
    const option = document.createElement('option')
    option.value = track.name
    option.textContent = formatCatalogTrackLabel(track, kind)
    option.selected = track.name === selectedTrackName
    select.appendChild(option)
  }
  setSelectedCatalogTrack(kind, selectedTrackName)
  return true
}

function renderCatalogTracks(): void {
  const hasVideoTracks = renderCatalogTrackSelect('video')
  const hasAudioTracks = renderCatalogTrackSelect('audio')

  if (!hasVideoTracks && !hasAudioTracks) {
    setCatalogTrackStatus('Catalog not loaded yet')
    return
  }
  setCatalogTrackStatus(`Catalog loaded: video=${catalogVideoTracks.length}, audio=${catalogAudioTracks.length}`)
}

function setupCatalogSelectionHandler(): void {
  const videoSelect = getCatalogTrackSelect('video')
  if (videoSelect) {
    videoSelect.addEventListener('change', () => {
      const value = videoSelect.value.trim()
      setSelectedCatalogTrack('video', value.length > 0 ? value : null)
    })
  }
  const audioSelect = getCatalogTrackSelect('audio')
  if (audioSelect) {
    audioSelect.addEventListener('change', () => {
      const value = audioSelect.value.trim()
      setSelectedCatalogTrack('audio', value.length > 0 ? value : null)
    })
  }
  const bypassInput = document.getElementById('bypass-jitter-buffer') as HTMLInputElement | null
  if (bypassInput) {
    bypassInput.addEventListener('change', () => {
      applyDecoderWorkerConfig()
      console.info('[MediaSubscriber] decoder config updated', {
        bypassJitterBuffer: bypassInput.checked
      })
    })
  }
}

function setupCatalogCallbacks(trackAlias: bigint): void {
  moqtClient.setOnSubgroupObjectHandler(trackAlias, (groupId, subgroupStreamObject) => {
    const payloadBytes = subgroupStreamObject.objectPayload
    console.info('[MediaSubscriber] received catalog object', {
      trackAlias: trackAlias.toString(),
      groupId: groupId.toString(),
      subgroupId: subgroupStreamObject.subgroupId?.toString() ?? '0',
      objectIdDelta: subgroupStreamObject.objectIdDelta.toString(),
      payloadLength: subgroupStreamObject.objectPayloadLength
    })
    const payload = new TextDecoder().decode(payloadBytes)
    try {
      const parsed = parse_msf_catalog_json(payload)
      catalogVideoTracks = extractCatalogVideoTracks(parsed)
      catalogAudioTracks = extractCatalogAudioTracks(parsed)
      console.info('[MediaSubscriber] parsed catalog', {
        trackAlias: trackAlias.toString(),
        videoTracks: catalogVideoTracks.length,
        audioTracks: catalogAudioTracks.length
      })
      renderCatalogTracks()
    } catch (error) {
      console.error('[MediaSubscriber] failed to parse catalog', error)
      setCatalogTrackStatus('Catalog parse failed')
    }
  })
}

function sendSetupButtonClickHandler(): void {
  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement
  sendSetupBtn.addEventListener('click', async () => {
    const form = getFormElement()

    const versions = toBigUint64Array('0xff00000E')
    const maxSubscribeId = BigInt(form['max-subscribe-id'].value)

    await moqtClient.sendClientSetup(versions, maxSubscribeId)
    setSetupStatus('Setup acknowledged')
  })
}

function sendCatalogSubscribeButtonClickHandler(): void {
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
    setCatalogTrackStatus(`Catalog subscribe requested: ${catalogTrackName}`)
  })
}

function sendSubscribeButtonClickHandler(): void {
  const sendSubscribeBtn = document.getElementById('sendSubscribeBtn') as HTMLButtonElement
  sendSubscribeBtn.addEventListener('click', async () => {
    const form = getFormElement()
    const trackNamespace = parseTrackNamespace(form['subscribe-track-namespace'].value)
    const selectedVideoTrack = selectedVideoTrackName ?? ''
    const selectedAudioTrack = selectedAudioTrackName ?? ''
    const videoSubscribeId = BigInt(form['video-subscribe-id'].value)
    const audioSubscribeId = BigInt(form['audio-subscribe-id'].value)

    if (!selectedVideoTrack || !selectedAudioTrack) {
      setCatalogTrackStatus('Select video and audio tracks from catalog first')
      return
    }

    const videoTrackAlias = await moqtClient.subscribe(videoSubscribeId, trackNamespace, selectedVideoTrack, AUTH_INFO)
    form['video-track-alias'].value = videoTrackAlias.toString()
    setupClientObjectCallbacks('video', videoTrackAlias)

    const audioTrackAlias = await moqtClient.subscribe(audioSubscribeId, trackNamespace, selectedAudioTrack, AUTH_INFO)
    form['audio-track-alias'].value = audioTrackAlias.toString()
    setupClientObjectCallbacks('audio', audioTrackAlias)
    setTrackSubscribeStatus(`Track subscribe requested: video=${selectedVideoTrack}, audio=${selectedAudioTrack}`)
  })
}

function setupAudioDecoderWorker() {
  if (audioWorkerInitialized) return

  audioWorkerInitialized = true
  const audioGenerator = new MediaStreamTrackGenerator({ kind: 'audio' })
  const audioWriter = audioGenerator.writable.getWriter()
  const audioStream = new MediaStream([audioGenerator])
  const audioElement = document.getElementById('audio') as HTMLAudioElement
  audioElement.srcObject = audioStream
  applyDecoderWorkerConfig()
  audioDecoderWorker.onmessage = async (event: MessageEvent<AudioDecoderWorkerMessage>) => {
    const data = event.data
    if (data.type === 'bufferedObject') {
      setPlaybackObjectPosition('audio', data.groupId, data.objectId)
      return
    }
    if (data.type !== 'audioData') {
      return
    }
    await audioWriter.ready
    await audioWriter.write(data.audioData)
    data.audioData.close()
    if (!audioPlaybackStarted) {
      await audioElement.play()
      audioPlaybackStarted = true
    }
  }
}
function setupVideoDecoderWorker() {
  if (videoWorkerInitialized) return

  videoWorkerInitialized = true
  const videoGenerator = new MediaStreamTrackGenerator({ kind: 'video' })
  const videoWriter = videoGenerator.writable.getWriter()
  const videoStream = new MediaStream([videoGenerator])
  const videoElement = document.getElementById('video') as HTMLVideoElement
  videoElement.srcObject = videoStream
  applyDecoderWorkerConfig()
  videoDecoderWorker.onmessage = async (event: MessageEvent<VideoDecoderWorkerMessage>) => {
    const data = event.data
    if (data.type === 'bufferedObject') {
      setPlaybackObjectPosition('video', data.groupId, data.objectId)
      return
    }
    if (data.type !== 'frame') {
      return
    }
    const videoFrame = data.frame
    await videoWriter.ready
    await videoWriter.write(videoFrame)
    videoFrame.close()
    if (!videoPlaybackStarted) {
      await videoElement.play()
      videoPlaybackStarted = true
    }
  }
}

function setupVideoPlaybackStatus(): void {
  const videoElement = document.getElementById('video') as HTMLVideoElement
  const updatePlaybackStatus = (label: string) => {
    setPlaybackStatus(
      `${label}: readyState=${videoElement.readyState}, currentTime=${videoElement.currentTime.toFixed(2)}`
    )
  }

  videoElement.addEventListener('loadeddata', () => {
    updatePlaybackStatus('Loaded data')
  })
  videoElement.addEventListener('playing', () => {
    updatePlaybackStatus('Playing')
  })
  videoElement.addEventListener('timeupdate', () => {
    if (videoElement.currentTime > 0) {
      updatePlaybackStatus('Playing')
    }
  })
  videoElement.addEventListener('pause', () => {
    if (videoElement.currentTime === 0) {
      setPlaybackStatus('Playback paused')
    }
  })
}

function setupClientObjectCallbacks(type: 'video' | 'audio', trackAlias: bigint): void {
  const alias = trackAlias

  if (type === 'audio') {
    setupAudioDecoderWorker()
    moqtClient.setOnSubgroupObjectHandler(alias, (groupId, subgroupStreamObject) => {
      receivedAudioObjectCount += 1
      setReceiveStatus(
        `Received video objects: ${receivedVideoObjectCount}, audio objects: ${receivedAudioObjectCount}`
      )
      const payload = new Uint8Array(subgroupStreamObject.objectPayload)
      const locSummary = summarizeLocHeader(subgroupStreamObject.locHeader)
      if (locSummary.present && locSummary.extensionCount > 0) {
        console.debug('[MediaSubscriber] LoC object (audio)', {
          trackAlias: alias.toString(),
          groupId,
          objectId: subgroupStreamObject.objectId,
          loc: locSummary
        })
      }
      console.debug('[MediaSubscriber] recv audio object', {
        groupId,
        objectId: subgroupStreamObject.objectId,
        payloadLength: subgroupStreamObject.objectPayloadLength,
        payloadByteLength: payload.byteLength,
        status: subgroupStreamObject.objectStatus,
        loc: locSummary
      })
      audioDecoderWorker.postMessage(
        {
          groupId,
          subgroupStreamObject: {
            subgroupId: subgroupStreamObject.subgroupId,
            objectIdDelta: subgroupStreamObject.objectIdDelta,
            objectPayloadLength: subgroupStreamObject.objectPayloadLength,
            objectPayload: payload,
            objectStatus: subgroupStreamObject.objectStatus,
            locHeader: subgroupStreamObject.locHeader
          }
        },
        [payload.buffer]
      )
    })
    return
  }

  setupVideoDecoderWorker()
  moqtClient.setOnSubgroupObjectHandler(alias, (groupId, subgroupStreamObject) => {
    receivedVideoObjectCount += 1
    setReceiveStatus(`Received video objects: ${receivedVideoObjectCount}, audio objects: ${receivedAudioObjectCount}`)
    if (selectedVideoTrackName && selectedAudioTrackName) {
      setTrackSubscribeStatus(`Subscribed video=${selectedVideoTrackName}, audio=${selectedAudioTrackName}`)
    }
    const payload = new Uint8Array(subgroupStreamObject.objectPayload)
    const locSummary = summarizeLocHeader(subgroupStreamObject.locHeader)
    if (locSummary.present && locSummary.extensionCount > 0) {
      console.debug('[MediaSubscriber] LoC object (video)', {
        trackAlias: alias.toString(),
        groupId,
        objectId: subgroupStreamObject.objectId,
        loc: locSummary
      })
    }
    console.debug('[MediaSubscriber] recv video object', {
      groupId,
      objectId: subgroupStreamObject.objectId,
      payloadLength: subgroupStreamObject.objectPayloadLength,
      payloadByteLength: payload.byteLength,
      status: subgroupStreamObject.objectStatus,
      loc: locSummary
    })

    videoDecoderWorker.postMessage(
      {
        groupId,
        subgroupStreamObject: {
          subgroupId: subgroupStreamObject.subgroupId,
          objectIdDelta: subgroupStreamObject.objectIdDelta,
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

moqtClient.setOnServerSetupHandler((serverSetup: any) => {
  console.log({ serverSetup })
  setSetupStatus('Setup acknowledged')
})

moqtClient.setOnSubscribeResponseHandler((subscribeResponse) => {
  console.log({ subscribeResponse })
})

function setupCloseButtonHandler(): void {
  const closeBtn = document.getElementById('closeBtn') as HTMLButtonElement
  closeBtn.addEventListener('click', async () => {
    await moqtClient.disconnect()
    audioPlaybackStarted = false
    videoPlaybackStarted = false
    moqtClient.clearSubgroupObjectHandlers()
    catalogVideoTracks = []
    catalogAudioTracks = []
    selectedVideoTrackName = null
    selectedAudioTrackName = null
    setPlaybackObjectStatus('video', 'Waiting for objects')
    setPlaybackObjectStatus('audio', 'Waiting for objects')
    receivedVideoObjectCount = 0
    receivedAudioObjectCount = 0
    renderCatalogTracks()
    setConnectionStatus('Disconnected')
    setSetupStatus('Setup not sent')
    setTrackSubscribeStatus('Subscription idle')
    setReceiveStatus('Waiting for media objects')
    setPlaybackStatus('Playback idle')
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

const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
connectBtn.addEventListener('click', async () => {
  const form = getFormElement()
  const url = form.url.value

  await moqtClient.connect(url, { sendSetup: false })
  receivedVideoObjectCount = 0
  receivedAudioObjectCount = 0
  setConnectionStatus(`Connected: ${url}`)
  setReceiveStatus('Waiting for media objects')
})

initializeMediaExamplePage('subscribe-track-namespace')
initializeStatuses()
setupButtonHandlers()
applyDecoderWorkerConfig()
setupVideoPlaybackStatus()
renderCatalogTracks()
setPlaybackObjectStatus('video', 'Waiting for objects')
setPlaybackObjectStatus('audio', 'Waiting for objects')
