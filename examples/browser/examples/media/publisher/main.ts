import { MoqtClientWrapper } from '@moqt/moqtClient'
import type { MOQTClient } from '../../../pkg/moqt_client_wasm'
import { AUTH_INFO } from './const'
import { sendVideoObjectMessage } from './sender'
import { getFormElement } from './utils'
import { MediaTransportState } from '../../../utils/media/transportState'
import { sendVideoChunkViaMoqt } from '../../../utils/media/videoTransport'
import { sendAudioChunkViaMoqt } from '../../../utils/media/audioTransport'
import { KEYFRAME_INTERVAL } from '../../../utils/media/constants'
import {
  MEDIA_AUDIO_PROFILES,
  MEDIA_CATALOG_TRACK_NAME,
  buildMediaCatalogJson,
  getMediaVideoProfiles
} from '../catalog'
import {
  getErrorMessage,
  getMediaVideoEncodingOverrides,
  initializeMediaExamplePage,
  parseTrackNamespace,
  setStatusText
} from '../common'

let mediaStream: MediaStream | null = null
const ENABLE_AUDIO_PUBLISH = false
const moqtClient = new MoqtClientWrapper()
let transportState = new MediaTransportState()
const catalogAliases = new Set<string>()
let setupCompleted = false
const activeSubscriberTracks = new Set<string>()
let handlersInitialized = false
let publishingStarted = false
let firstVideoChunkSent = false
let firstAudioChunkSent = false

function ensureClient(): MOQTClient {
  const client = moqtClient.getRawClient()
  if (!client) {
    throw new Error('MOQT client not connected')
  }
  return client
}

function setConnectionStatus(text: string): void {
  setStatusText('publisher-connection-status', text)
}

function setSetupStatus(text: string): void {
  setStatusText('publisher-setup-status', text)
}

function setAnnounceStatus(text: string): void {
  setStatusText('publisher-announce-status', text)
}

function setCaptureStatus(text: string): void {
  setStatusText('publisher-capture-status', text)
}

function setSendStatus(text: string): void {
  setStatusText('publisher-send-status', text)
}

function initializeStatuses(): void {
  setConnectionStatus('Not connected')
  setSetupStatus('Setup not sent')
  setAnnounceStatus('Announce not sent')
  setCaptureStatus('Capture idle')
  setSendStatus('Waiting for subscribers')
}

function setUpStartGetUserMediaButton() {
  const startGetUserMediaBtn = document.getElementById('startGetUserMediaBtn') as HTMLButtonElement
  const getSampleVideoBtn = document.getElementById('getSampleVideo') as HTMLButtonElement

  const startCapture = async (source: 'camera' | 'sample') => {
    setCaptureStatus(`Requesting ${source === 'sample' ? 'sample media' : 'camera and microphone'}`)
    const constraints = {
      audio: true,
      video: {
        width: { ideal: 1920 },
        height: { ideal: 1080 }
      }
    }
    try {
      mediaStream = await navigator.mediaDevices.getUserMedia(constraints)
      const video = document.getElementById('video') as HTMLVideoElement
      video.srcObject = mediaStream
      const [videoTrack] = mediaStream.getVideoTracks()
      const settings = videoTrack?.getSettings() ?? {}
      const dimensions =
        typeof settings.width === 'number' && typeof settings.height === 'number'
          ? `${settings.width}x${settings.height}`
          : 'unknown resolution'
      setCaptureStatus(`Media ready (${source}): ${dimensions}`)
    } catch (error) {
      setCaptureStatus(`getUserMedia failed: ${getErrorMessage(error)}`)
      throw error
    }
  }

  startGetUserMediaBtn.addEventListener('click', async () => {
    await startCapture('camera')
  })
  getSampleVideoBtn.addEventListener('click', async () => {
    await startCapture('sample')
  })
}

const videoEncoderWorker = new Worker(new URL('../../../utils/media/encoders/videoEncoder.ts', import.meta.url), {
  type: 'module'
})
type AudioProfileEncoderContext = {
  bitrate: number
  trackName: string
  worker: Worker
}

const audioEncoderContexts: AudioProfileEncoderContext[] = MEDIA_AUDIO_PROFILES.map((profile) => ({
  bitrate: profile.bitrate,
  trackName: profile.trackName,
  worker: new Worker(new URL('../../../utils/media/encoders/audioEncoder.ts', import.meta.url), {
    type: 'module'
  })
}))

type VideoEncoderWorkerMessage =
  | {
      type: 'chunk'
      chunk: EncodedVideoChunk
      metadata: EncodedVideoChunkMetadata | undefined
      captureTimestampMicros?: number
    }
  | { type: 'bitrate'; kbps: number }

type AudioEncoderWorkerMessage =
  | {
      type: 'chunk'
      chunk: EncodedAudioChunk
      metadata: EncodedAudioChunkMetadata | undefined
      captureTimestampMicros?: number
    }
  | { type: 'bitrate'; media: 'audio'; kbps: number }

function getForwardingPreference(form: HTMLFormElement): string {
  return (Array.from(form['forwarding-preference']) as HTMLInputElement[]).filter((elem) => elem.checked)[0].value
}

function isCatalogTrack(trackName: string): boolean {
  return trackName === MEDIA_CATALOG_TRACK_NAME
}

function isVideoTrack(trackName: string): boolean {
  return getMediaVideoProfiles().some((profile) => profile.trackName === trackName)
}

function isAudioTrack(trackName: string): boolean {
  return MEDIA_AUDIO_PROFILES.some((profile) => profile.trackName === trackName)
}

function getVideoTrackAliases(client: MOQTClient, trackNamespace: string[]): bigint[] {
  const aliases = new Set<string>()
  for (const profile of getMediaVideoProfiles()) {
    for (const alias of client.getTrackSubscribers(trackNamespace, profile.trackName)) {
      aliases.add(alias.toString())
    }
  }
  return Array.from(aliases, (alias) => BigInt(alias))
}

function configureVideoEncoderForCurrentPage(videoTrack: MediaStreamTrack): void {
  const { codec, hardwareAcceleration } = getMediaVideoEncodingOverrides()
  if (!codec && !hardwareAcceleration) {
    return
  }

  const settings = videoTrack.getSettings()
  videoEncoderWorker.postMessage({
    type: 'encoderConfig',
    config: {
      codec: codec ?? 'avc1.640028',
      width: typeof settings.width === 'number' ? settings.width : 1280,
      height: typeof settings.height === 'number' ? settings.height : 720,
      bitrate: 1_000_000,
      framerate: typeof settings.frameRate === 'number' ? settings.frameRate : 30,
      hardwareAcceleration
    }
  })
}

function getAudioTrackAliases(client: MOQTClient, trackNamespace: string[], trackName: string): bigint[] {
  return Array.from(client.getTrackSubscribers(trackNamespace, trackName), (alias) => BigInt(alias))
}

async function sendCatalog(client: MOQTClient, trackAlias: bigint, trackNamespace: string[]): Promise<void> {
  const aliasKey = trackAlias.toString()
  if (catalogAliases.has(aliasKey)) {
    console.info('[MediaPublisher] catalog already sent', { trackAlias: aliasKey, trackNamespace })
    return
  }
  const payload = new TextEncoder().encode(buildMediaCatalogJson(trackNamespace))
  console.info('[MediaPublisher] sending catalog', {
    trackAlias: aliasKey,
    trackNamespace,
    groupId: '0',
    subgroupId: '0',
    objectId: '0',
    payloadLength: payload.byteLength
  })
  await client.sendSubgroupHeader(trackAlias, 0n, 0n, 0)
  await client.sendSubgroupObject(trackAlias, 0n, 0n, 0n, undefined, payload, undefined)
  catalogAliases.add(aliasKey)
  setSendStatus(`Catalog served: ${trackNamespace.join('/')}/${MEDIA_CATALOG_TRACK_NAME}`)
  console.info('[MediaPublisher] sent catalog', { trackAlias: aliasKey, trackNamespace })
}

function sendSetupButtonClickHandler(): void {
  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement
  sendSetupBtn.addEventListener('click', async () => {
    try {
      const form = getFormElement()
      const versions = new BigUint64Array('0xff00000E'.split(',').map(BigInt))
      const maxSubscribeId = BigInt(form['max-subscribe-id'].value)

      await moqtClient.sendClientSetup(versions, maxSubscribeId)
      setSetupStatus('Setup acknowledged')
      console.info('[MediaPublisher] CLIENT_SETUP sent', { maxSubscribeId: maxSubscribeId.toString() })
    } catch (error) {
      setSetupStatus(`Setup failed: ${getErrorMessage(error)}`)
      console.error('[MediaPublisher] CLIENT_SETUP failed', error)
    }
  })
}

function sendPublishNamespaceButtonClickHandler(): void {
  const sendPublishNamespaceBtn = document.getElementById('sendPublishNamespaceBtn') as HTMLButtonElement
  sendPublishNamespaceBtn.addEventListener('click', async () => {
    try {
      if (!moqtClient.getConnectionStatus()) {
        console.warn('[MediaPublisher] Connect first before sending PUBLISH_NAMESPACE')
        return
      }
      if (!setupCompleted) {
        console.warn('[MediaPublisher] Send CLIENT_SETUP first before sending PUBLISH_NAMESPACE')
        return
      }

      const form = getFormElement()
      const trackNamespace = parseTrackNamespace(form['publish-track-namespace'].value)

      console.info('[MediaPublisher] sending PUBLISH_NAMESPACE', { trackNamespace })
      await moqtClient.publishNamespace(trackNamespace, AUTH_INFO)
      setAnnounceStatus(`Announced: ${trackNamespace.join('/')}`)
      console.info('[MediaPublisher] PUBLISH_NAMESPACE completed', { trackNamespace })
    } catch (error) {
      setAnnounceStatus(`Announce failed: ${getErrorMessage(error)}`)
      console.error('[MediaPublisher] PUBLISH_NAMESPACE failed', error)
    }
  })
}

function sendSubgroupObjectButtonClickHandler(): void {
  const sendSubgroupObjectBtn = document.getElementById('sendSubgroupObjectBtn') as HTMLButtonElement
  sendSubgroupObjectBtn.addEventListener('click', async () => {
    console.log('clicked sendSubgroupObjectBtn')
    if (mediaStream == null) {
      setSendStatus('Media stream is not ready')
      console.error('mediaStream is null')
      return
    }
    if (publishingStarted) {
      setSendStatus('Publishing already started')
      return
    }
    let client: MOQTClient
    try {
      client = ensureClient()
    } catch (error) {
      setSendStatus(`Publish failed: ${getErrorMessage(error)}`)
      console.error(error)
      return
    }
    publishingStarted = true
    setSendStatus('Publishing started')

    videoEncoderWorker.onmessage = async (event: MessageEvent<VideoEncoderWorkerMessage>) => {
      const data = event.data
      if (data.type !== 'chunk') {
        return
      }
      const { chunk, metadata, captureTimestampMicros } = data
      const form = getFormElement()
      const trackNamespace = parseTrackNamespace(form['publish-track-namespace'].value)
      const publisherPriority = Number(form['video-publisher-priority'].value)
      const trackAliases = getVideoTrackAliases(client, trackNamespace)
      if (!trackAliases.length) {
        return
      }
      if (!firstVideoChunkSent) {
        firstVideoChunkSent = true
        setSendStatus('Streaming video chunks')
      }

      await sendVideoChunkViaMoqt({
        chunk,
        metadata,
        captureTimestampMicros,
        trackAliases,
        publisherPriority,
        client,
        transportState,
        sender: sendVideoObjectMessage
      })
    }
    if (ENABLE_AUDIO_PUBLISH) {
      for (const context of audioEncoderContexts) {
        context.worker.onmessage = async (event: MessageEvent<AudioEncoderWorkerMessage>) => {
          const data = event.data
          if (data.type !== 'chunk') {
            return
          }
          const { chunk, metadata, captureTimestampMicros } = data
          const form = getFormElement()
          const trackNamespace = parseTrackNamespace(form['publish-track-namespace'].value)
          const trackAliases = getAudioTrackAliases(client, trackNamespace, context.trackName)
          if (!trackAliases.length) {
            return
          }
          if (!firstAudioChunkSent) {
            firstAudioChunkSent = true
            setSendStatus('Streaming video and audio chunks')
          }
          await sendAudioChunkViaMoqt({
            chunk,
            metadata,
            captureTimestampMicros,
            trackAliases,
            client,
            transportState
          })
        }
      }
    }

    const [videoTrack] = mediaStream.getVideoTracks()
    configureVideoEncoderForCurrentPage(videoTrack)
    const videoProcessor = new MediaStreamTrackProcessor({ track: videoTrack })
    const videoStream = videoProcessor.readable
    transportState = new MediaTransportState()
    videoEncoderWorker.postMessage({
      type: 'keyframeInterval',
      keyframeInterval: KEYFRAME_INTERVAL
    })
    videoEncoderWorker.postMessage({ type: 'videoStream', videoStream: videoStream }, [videoStream])
    if (ENABLE_AUDIO_PUBLISH) {
      const [audioTrack] = mediaStream.getAudioTracks()
      audioEncoderContexts.forEach((context, index) => {
        context.worker.postMessage({
          type: 'config',
          config: { bitrate: context.bitrate }
        })
        const sourceTrack = index === 0 ? audioTrack : audioTrack.clone()
        const audioProcessor = new MediaStreamTrackProcessor({ track: sourceTrack })
        const audioStream = audioProcessor.readable
        context.worker.postMessage({ type: 'audioStream', audioStream: audioStream }, [audioStream])
      })
    }
  })
}

function setupButtonClickHandler(): void {
  if (handlersInitialized) {
    return
  }
  sendSetupButtonClickHandler()
  sendPublishNamespaceButtonClickHandler()
  sendSubgroupObjectButtonClickHandler()
  handlersInitialized = true
}

function setupClientCallbacks(): void {
  moqtClient.setOnServerSetupHandler((serverSetup: any) => {
    setupCompleted = true
    console.log({ serverSetup })
    setSetupStatus('Setup acknowledged')
  })

  moqtClient.setOnPublishNamespaceResponseHandler((responseMessage) => {
    console.log({ publishNamespaceResponseMessage: responseMessage })
  })

  moqtClient.setOnPublishNamespaceHandler(async ({ publishNamespace, respondOk }) => {
    console.log({ publishNamespace })
    await respondOk()
  })

  moqtClient.setOnSubscribeResponseHandler((subscribeResponseMessage) => {
    console.log({ subscribeResponseMessage })
  })

  moqtClient.setOnIncomingSubscribeHandler(async ({ subscribe, isSuccess, code, respondOk, respondError }) => {
    console.log({ subscribeMessage: subscribe })
    const form = getFormElement()
    const trackNamespace = parseTrackNamespace(form['publish-track-namespace'].value)
    const requestedNamespace = subscribe.trackNamespace ?? []
    const trackName = subscribe.trackName ?? ''
    const namespaceMatched = requestedNamespace.join('/') === trackNamespace.join('/')

    if (isSuccess) {
      if (!namespaceMatched) {
        setSendStatus(`Rejected subscribe: unknown namespace ${requestedNamespace.join('/')}`)
        await respondError(404n, 'unknown namespace')
        return
      }
      if (isCatalogTrack(trackName)) {
        const trackAlias = await respondOk(0n)
        const client = ensureClient()
        await sendCatalog(client, trackAlias, trackNamespace)
        return
      }
      if (isVideoTrack(trackName) || isAudioTrack(trackName)) {
        await respondOk(0n)
        activeSubscriberTracks.add(trackName)
        setSendStatus(`Subscriber ready: ${Array.from(activeSubscriberTracks).sort().join(', ')}`)
        return
      }
      setSendStatus(`Rejected subscribe: unknown track ${trackName}`)
      await respondError(404n, 'unknown track')
      return
    }

    const reasonPhrase = `subscribe error: code=${code}`
    setSendStatus(reasonPhrase)
    await respondError(BigInt(code), reasonPhrase)
  })
}

function setupCloseButtonHandler(): void {
  const closeBtn = document.getElementById('closeBtn') as HTMLButtonElement
  closeBtn.addEventListener('click', async () => {
    await moqtClient.disconnect()
    catalogAliases.clear()
    activeSubscriberTracks.clear()
    mediaStream = null
    publishingStarted = false
    firstVideoChunkSent = false
    firstAudioChunkSent = false
    setConnectionStatus('Disconnected')
    setSetupStatus('Setup not sent')
    setAnnounceStatus('Announce not sent')
    setCaptureStatus('Capture idle')
    setSendStatus('Waiting for subscribers')
  })
}

setUpStartGetUserMediaButton()
setupClientCallbacks()
setupCloseButtonHandler()
initializeMediaExamplePage('publish-track-namespace')
initializeStatuses()

const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
connectBtn.addEventListener('click', async () => {
  const form = getFormElement()
  const url = form.url.value

  setupCompleted = false
  await moqtClient.connect(url, { sendSetup: false })
  catalogAliases.clear()
  console.info('[MediaPublisher] connected', { url })
  activeSubscriberTracks.clear()
  publishingStarted = false
  firstVideoChunkSent = false
  firstAudioChunkSent = false
  setupButtonClickHandler()
  setConnectionStatus(`Connected: ${url}`)
  setSendStatus('Waiting for subscribers')
})

setupButtonClickHandler()
