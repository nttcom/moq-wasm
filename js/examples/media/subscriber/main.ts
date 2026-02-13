import { MoqtClientWrapper } from '@moqt/moqtClient'
import { parse_msf_catalog_json } from '../../../pkg/moqt_client_wasm'
import { AUTH_INFO } from './const'
import { getFormElement } from './utils'
import { summarizeLocHeader } from '../../../utils/media/locSummary'
import { extractCatalogAudioTracks, extractCatalogVideoTracks, type MediaCatalogTrack } from '../catalog'

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

type VideoDecoderWorkerMessage =
  | { type: 'frame'; frame: VideoFrame }
  | { type: 'bitrate'; kbps: number }
  | { type: 'receiveLatency'; media: 'video'; ms: number }
  | { type: 'renderingLatency'; media: 'video'; ms: number }

let audioWorkerInitialized = false
let videoWorkerInitialized = false
let handlersInitialized = false
let catalogVideoTracks: MediaCatalogTrack[] = []
let catalogAudioTracks: MediaCatalogTrack[] = []
let selectedVideoTrackName: string | null = null
let selectedAudioTrackName: string | null = null

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
  if (trackName && track?.codec) {
    videoDecoderWorker.postMessage({ type: 'catalog', codec: track.codec })
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
    setCatalogTrackStatus('')
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
}

function setupCatalogCallbacks(trackAlias: bigint): void {
  moqtClient.setOnSubgroupObjectHandler(trackAlias, (_groupId, subgroupStreamObject) => {
    const payload = new TextDecoder().decode(new Uint8Array(subgroupStreamObject.objectPayload))
    try {
      const parsed = parse_msf_catalog_json(payload)
      catalogVideoTracks = extractCatalogVideoTracks(parsed)
      catalogAudioTracks = extractCatalogAudioTracks(parsed)
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
    const selectedAudioTrack = selectedAudioTrackName ?? ''
    const videoSubscribeId = BigInt(form['video-subscribe-id'].value)
    const videoTrackAlias = BigInt(form['video-track-alias'].value)
    const audioSubscribeId = BigInt(form['audio-subscribe-id'].value)
    const audioTrackAlias = BigInt(form['audio-track-alias'].value)

    if (!selectedVideoTrack || !selectedAudioTrack) {
      setCatalogTrackStatus('Select video and audio tracks from catalog first')
      return
    }

    setupClientObjectCallbacks('video', videoTrackAlias)
    await moqtClient.subscribe(videoSubscribeId, videoTrackAlias, trackNamespace, selectedVideoTrack, AUTH_INFO)

    setupClientObjectCallbacks('audio', audioTrackAlias)
    await moqtClient.subscribe(audioSubscribeId, audioTrackAlias, trackNamespace, selectedAudioTrack, AUTH_INFO)
    setCatalogTrackStatus(`Subscribed video=${selectedVideoTrack}, audio=${selectedAudioTrack}`)
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
  audioDecoderWorker.onmessage = async (event: MessageEvent<AudioDecoderWorkerMessage>) => {
    const data = event.data
    if (data.type !== 'audioData') {
      console.debug('[MediaSubscriber] audio worker event', data)
      return
    }
    await audioWriter.ready
    await audioWriter.write(data.audioData)
    await audioElement.play()
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
  videoDecoderWorker.onmessage = async (event: MessageEvent<VideoDecoderWorkerMessage>) => {
    const data = event.data
    if (data.type !== 'frame') {
      console.debug('[MediaSubscriber] video worker event', data)
      return
    }
    const videoFrame = data.frame
    await videoWriter.ready
    await videoWriter.write(videoFrame)
    videoFrame.close()
    await videoElement.play()
  }
}

function setupClientObjectCallbacks(type: 'video' | 'audio', trackAlias: bigint): void {
  const alias = trackAlias

  if (type === 'audio') {
    setupAudioDecoderWorker()
    moqtClient.setOnSubgroupObjectHandler(alias, (groupId, subgroupStreamObject) => {
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
    return
  }

  setupVideoDecoderWorker()
  moqtClient.setOnSubgroupObjectHandler(alias, (groupId, subgroupStreamObject) => {
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

moqtClient.setOnServerSetupHandler((serverSetup: any) => {
  console.log({ serverSetup })
})

moqtClient.setOnSubscribeResponseHandler((subscribeResponse) => {
  console.log({ subscribeResponse })
})

function setupCloseButtonHandler(): void {
  const closeBtn = document.getElementById('closeBtn') as HTMLButtonElement
  closeBtn.addEventListener('click', async () => {
    await moqtClient.disconnect()
    moqtClient.clearSubgroupObjectHandlers()
    catalogVideoTracks = []
    catalogAudioTracks = []
    selectedVideoTrackName = null
    selectedAudioTrackName = null
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

const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
connectBtn.addEventListener('click', async () => {
  const form = getFormElement()
  const url = form.url.value

  await moqtClient.connect(url, { sendSetup: false })
  setCatalogTrackStatus('Connected')
})

setupButtonHandlers()
renderCatalogTracks()
