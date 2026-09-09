import { MoqtClientWrapper } from '@moqt/moqtClient'
import { parse_msf_catalog_json } from '../../pkg/moqt_client_wasm'
import { parseIngestChunk } from '../../utils/media/ingestChunk'
import {
  MEDIA_CATALOG_TRACK_NAME,
  extractCatalogAudioTracks,
  extractCatalogVideoTracks,
  type MediaCatalogTrack
} from '../media/catalog'
import { getErrorMessage, initializeMediaExamplePage, parseTrackNamespace, setStatusText } from '../media/common'

const AUTH_INFO = 'secret'
const ANNEX_B_FORMAT = 'annexb'

type MediaKind = 'video' | 'audio'

type TrackSubscription = {
  requestId: bigint
  trackAlias: bigint
  name: string
}

const moqtClient = new MoqtClientWrapper()
const videoDecoderWorker = new Worker(new URL('../../utils/media/decoders/videoDecoder.ts', import.meta.url), {
  type: 'module'
})
const audioDecoderWorker = new Worker(new URL('../../utils/media/decoders/audioDecoder.ts', import.meta.url), {
  type: 'module'
})

let videoTracks: MediaCatalogTrack[] = []
let audioTracks: MediaCatalogTrack[] = []
const subscriptions = new Map<MediaKind, TrackSubscription>()
let videoWriter: WritableStreamDefaultWriter<VideoFrame> | undefined
let audioWriter: WritableStreamDefaultWriter<AudioData> | undefined
let videoObjectCount = 0
let receivedKbps = 0

initializeMediaExamplePage('namespace')
element<HTMLButtonElement>('watchBtn').addEventListener('click', () => void watchStream())
element<HTMLButtonElement>('stopBtn').addEventListener('click', () => void stopStream())
element<HTMLSelectElement>('video-track').addEventListener('change', () => void resubscribe('video'))
element<HTMLSelectElement>('audio-track').addEventListener('change', () => void resubscribe('audio'))
element<HTMLInputElement>('bypass-jitter-buffer').addEventListener('change', applyDecoderConfig)
startRendering()

async function watchStream(): Promise<void> {
  try {
    await stopStream()
    const url = element<HTMLInputElement>('url').value.trim()
    await moqtClient.connect(url)
    setStatusText('connection-status', `Connected: ${url}`)
    appendLog('info', `connected to ${url}`)
    await subscribeCatalog()
  } catch (error) {
    setStatusText('connection-status', `Failed: ${getErrorMessage(error)}`)
    appendLog('error', getErrorMessage(error))
  }
}

async function stopStream(): Promise<void> {
  for (const kind of subscriptions.keys()) {
    await unsubscribeTrack(kind)
  }
  videoTracks = []
  audioTracks = []
  renderTrackOptions()
  videoObjectCount = 0
  if (moqtClient.getConnectionStatus()) {
    await moqtClient.disconnect()
  }
  setStatusText('connection-status', 'Not connected')
  setStatusText('catalog-status', 'Catalog not loaded yet')
  setStatusText('playback-status', 'Playback idle')
}

async function subscribeCatalog(): Promise<void> {
  const namespace = trackNamespace()
  const { subscribeOk } = await moqtClient.subscribe(namespace, MEDIA_CATALOG_TRACK_NAME, AUTH_INFO, {
    forward: true
  })
  moqtClient.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (_groupId, object) => {
    const { data } = parseIngestChunk(new Uint8Array(object.objectPayload))
    if (data.byteLength === 0) {
      return
    }
    void applyCatalog(new TextDecoder().decode(data))
  })
  appendLog('info', `subscribed ${namespace.join('/')}/${MEDIA_CATALOG_TRACK_NAME}`)
}

async function applyCatalog(payload: string): Promise<void> {
  try {
    const catalog = parse_msf_catalog_json(payload)
    videoTracks = extractCatalogVideoTracks(catalog)
    audioTracks = extractCatalogAudioTracks(catalog)
    setStatusText('catalog-status', `Catalog loaded: ${videoTracks.length} video / ${audioTracks.length} audio`)
    const changed = renderTrackOptions()
    if (changed) {
      await resubscribe('video')
      await resubscribe('audio')
    }
  } catch (error) {
    setStatusText('catalog-status', `Catalog error: ${getErrorMessage(error)}`)
    appendLog('error', `catalog: ${getErrorMessage(error)}`)
  }
}

function renderTrackOptions(): boolean {
  const videoChanged = fillSelect(element<HTMLSelectElement>('video-track'), videoTracks, describeVideoTrack)
  const audioChanged = fillSelect(element<HTMLSelectElement>('audio-track'), audioTracks, (track) => track.label)
  return videoChanged || audioChanged
}

function fillSelect(
  select: HTMLSelectElement,
  tracks: MediaCatalogTrack[],
  describe: (track: MediaCatalogTrack) => string
): boolean {
  const names = tracks.map((track) => track.name)
  const current = Array.from(select.options).map((option) => option.value)
  if (names.length === current.length && names.every((name, index) => name === current[index])) {
    return false
  }

  const selected = select.value
  select.replaceChildren(
    ...tracks.map((track) => {
      const option = document.createElement('option')
      option.value = track.name
      option.textContent = describe(track)
      return option
    })
  )
  select.value = names.includes(selected) ? selected : (names[0] ?? '')
  return true
}

function describeVideoTrack(track: MediaCatalogTrack): string {
  const resolution = track.width && track.height ? ` (${track.width}x${track.height})` : ''
  return `${track.label}${resolution}`
}

async function resubscribe(kind: MediaKind): Promise<void> {
  const select = element<HTMLSelectElement>(kind === 'video' ? 'video-track' : 'audio-track')
  const trackName = select.value
  if (subscriptions.get(kind)?.name === trackName) {
    return
  }

  await unsubscribeTrack(kind)
  const track = (kind === 'video' ? videoTracks : audioTracks).find((candidate) => candidate.name === trackName)
  if (!track) {
    return
  }

  postCatalogToDecoder(kind, track)
  const { requestId, subscribeOk } = await moqtClient.subscribe(trackNamespace(), trackName, AUTH_INFO, {
    forward: true
  })
  subscriptions.set(kind, { requestId, trackAlias: subscribeOk.trackAlias, name: trackName })
  const worker = kind === 'video' ? videoDecoderWorker : audioDecoderWorker
  moqtClient.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (groupId, object) => {
    const chunk = parseIngestChunk(new Uint8Array(object.objectPayload), object.locHeader)
    if (kind === 'video') {
      videoObjectCount += 1
      setStatusText('playback-status', `Playing ${trackName}`)
    }
    worker.postMessage(
      {
        groupId,
        subgroupStreamObject: {
          subgroupId: object.subgroupId,
          objectIdDelta: object.objectIdDelta,
          objectPayloadLength: chunk.data.byteLength,
          objectPayload: chunk.data,
          objectStatus: object.objectStatus,
          locHeader: chunk.locHeader
        }
      },
      [chunk.data.buffer]
    )
  })
  appendLog('info', `subscribed ${trackNamespace().join('/')}/${trackName}`)
}

async function unsubscribeTrack(kind: MediaKind): Promise<void> {
  const subscription = subscriptions.get(kind)
  if (!subscription) {
    return
  }

  subscriptions.delete(kind)
  moqtClient.clearSubgroupObjectHandler(subscription.trackAlias)
  if (moqtClient.getConnectionStatus()) {
    await moqtClient.unsubscribe(subscription.requestId)
  }
  appendLog('info', `unsubscribed ${subscription.name}`)
}

function postCatalogToDecoder(kind: MediaKind, track: MediaCatalogTrack): void {
  if (kind === 'video') {
    videoDecoderWorker.postMessage({
      type: 'catalog',
      codec: track.codec,
      descriptionBase64: track.initData,
      avcFormat: track.initData ? undefined : ANNEX_B_FORMAT
    })
    return
  }
  audioDecoderWorker.postMessage({
    type: 'catalog',
    codec: track.codec,
    sampleRate: track.samplerate,
    channels: channelCount(track.channelConfig),
    descriptionBase64: track.initData
  })
}

function channelCount(channelConfig?: string): number | undefined {
  if (channelConfig === 'mono') {
    return 1
  }
  if (channelConfig === 'stereo') {
    return 2
  }
  const parsed = Number.parseInt(channelConfig ?? '', 10)
  return Number.isNaN(parsed) ? undefined : parsed
}

function applyDecoderConfig(): void {
  const config = {
    telemetryEnabled: true,
    bypassJitterBuffer: element<HTMLInputElement>('bypass-jitter-buffer').checked
  }
  videoDecoderWorker.postMessage({ type: 'config', config })
  audioDecoderWorker.postMessage({ type: 'config', config })
}

function startRendering(): void {
  applyDecoderConfig()
  const videoGenerator = new MediaStreamTrackGenerator({ kind: 'video' })
  const audioGenerator = new MediaStreamTrackGenerator({ kind: 'audio' })
  videoWriter = videoGenerator.writable.getWriter()
  audioWriter = audioGenerator.writable.getWriter()
  element<HTMLVideoElement>('video').srcObject = new MediaStream([videoGenerator])
  element<HTMLAudioElement>('audio').srcObject = new MediaStream([audioGenerator])

  videoDecoderWorker.onmessage = async (event) => {
    if (event.data.type === 'bitrate') {
      receivedKbps = event.data.kbps ?? receivedKbps
      return
    }
    if (event.data.type !== 'frame') {
      return
    }
    const frame = event.data.frame as VideoFrame
    updateVideoStats(frame)
    if (!videoWriter || videoWriter.desiredSize === null || videoWriter.desiredSize <= 0) {
      frame.close()
      return
    }
    await videoWriter.ready
    await videoWriter.write(frame)
    frame.close()
  }

  audioDecoderWorker.onmessage = async (event) => {
    if (event.data.type !== 'audioData') {
      return
    }
    const audioData = event.data.audioData as AudioData
    if (!audioWriter) {
      audioData.close()
      return
    }
    await audioWriter.ready
    await audioWriter.write(audioData)
    audioData.close()
  }
}

function updateVideoStats(frame: VideoFrame): void {
  const stats = element<HTMLSpanElement>('video-stats')
  stats.textContent = `${frame.displayWidth}x${frame.displayHeight} · ${Math.round(receivedKbps)} kbps · ${videoObjectCount} objects`
}

function trackNamespace(): string[] {
  return parseTrackNamespace(element<HTMLInputElement>('namespace').value)
}

function appendLog(level: 'info' | 'warn' | 'error', message: string): void {
  const panel = document.getElementById('logPanel')
  if (!panel) {
    return
  }

  const entry = document.createElement('div')
  entry.className = `log-entry log-${level}`
  entry.textContent = `${new Date().toLocaleTimeString()} ${message}`
  panel.prepend(entry)
}

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id)
  if (!found) {
    throw new Error(`missing element: ${id}`)
  }
  return found as T
}
