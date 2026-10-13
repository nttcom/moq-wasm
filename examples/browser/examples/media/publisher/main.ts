import { MoqtClientWrapper } from '@moqt/moqtClient'
import type { MOQTClient } from '../../../pkg/moqt'
import { MediaTransportState } from '../../../utils/media/transportState'
import { timeSeededGroupId } from '../../../utils/media/clock'
import { sendVideoChunkViaMoqt } from '../../../utils/media/videoTransport'
import { buildMediaCatalogJson, getMediaVideoProfiles } from '../catalog'
import { getMediaVideoEncodingOverrides } from '../common'
import { element, getErrorMessage, parseTrackNamespace, setStatusText, initializeMediaExamplePage } from '../../../utils/page'
import { CATALOG_TRACK_NAME } from '../../../lib/msf/catalog'

/// A keyframe every 2 s at 30 fps.
const KEYFRAME_INTERVAL = 60
let mediaStream: MediaStream | null = null
const moqtClient = new MoqtClientWrapper()
const form = element<HTMLFormElement>('form')
let transportState = new MediaTransportState()
const catalogAliases = new Set<string>()
let nextCatalogGroupId = timeSeededGroupId()
let setupCompleted = false
const activeSubscriberTracks = new Set<string>()
let publishingStarted = false
let firstVideoChunkSent = false

function ensureClient(): MOQTClient {
  const client = moqtClient.getRawClient()
  if (!client) {
    throw new Error('MOQT client not connected')
  }
  return client
}

function initializeStatuses(): void {
  setStatusText('publisher-connection-status', 'Not connected')
  setStatusText('publisher-setup-status', 'Setup not sent')
  setStatusText('publisher-announce-status', 'Announce not sent')
  setStatusText('publisher-capture-status', 'Capture idle')
  setStatusText('publisher-send-status', 'Waiting for subscribers')
}

function setUpStartGetUserMediaButton() {
  const startGetUserMediaBtn = document.getElementById('startGetUserMediaBtn') as HTMLButtonElement

  const startCapture = async () => {
    setStatusText('publisher-capture-status', 'Requesting camera')
    const constraints = {
      audio: false,
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
      setStatusText('publisher-capture-status', `Media ready: ${dimensions}`)
    } catch (error) {
      setStatusText('publisher-capture-status', `getUserMedia failed: ${getErrorMessage(error)}`)
      throw error
    }
  }

  startGetUserMediaBtn.addEventListener('click', startCapture)
}

const videoEncoderWorker = new Worker(new URL('../../../utils/media/encoders/videoEncoder.ts', import.meta.url), {
  type: 'module'
})
type VideoEncoderWorkerMessage =
  | {
      type: 'chunk'
      chunk: EncodedVideoChunk
      metadata: EncodedVideoChunkMetadata | undefined
      captureTimestampMicros?: number
    }
  | { type: 'bitrate'; kbps: number }

function isCatalogTrack(trackName: string): boolean {
  return trackName === CATALOG_TRACK_NAME
}

function isVideoTrack(trackName: string): boolean {
  return getMediaVideoProfiles().some((profile) => profile.trackName === trackName)
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

async function sendCatalog(client: MOQTClient, trackAlias: bigint, trackNamespace: string[]): Promise<void> {
  const aliasKey = trackAlias.toString()
  if (catalogAliases.has(aliasKey)) {
    console.info('[MediaPublisher] catalog already sent', { trackAlias: aliasKey, trackNamespace })
    return
  }
  const payload = new TextEncoder().encode(buildMediaCatalogJson(trackNamespace, 'loc', getMediaVideoProfiles(), []))
  const groupId = nextCatalogGroupId++
  console.info('[MediaPublisher] sending catalog', {
    trackAlias: aliasKey,
    trackNamespace,
    groupId: groupId.toString(),
    subgroupId: '0',
    objectId: '0',
    payloadLength: payload.byteLength
  })
  await client.sendSubgroupHeader(trackAlias, groupId, 0n, 0)
  await client.sendSubgroupObject(trackAlias, groupId, 0n, 0n, undefined, payload, undefined)
  catalogAliases.add(aliasKey)
  setStatusText('publisher-send-status', `Catalog served: ${trackNamespace.join('/')}/${CATALOG_TRACK_NAME}`)
  console.info('[MediaPublisher] sent catalog', { trackAlias: aliasKey, trackNamespace })
}

function sendSetupButtonClickHandler(): void {
  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement
  sendSetupBtn.addEventListener('click', async () => {
    try {
      const versions = new BigUint64Array([0xff00000en])
      const maxSubscribeId = BigInt(form['max-subscribe-id'].value)

      await moqtClient.sendClientSetup(versions, maxSubscribeId)
      setStatusText('publisher-setup-status', 'Setup acknowledged')
      console.info('[MediaPublisher] CLIENT_SETUP sent', { maxSubscribeId: maxSubscribeId.toString() })
    } catch (error) {
      setStatusText('publisher-setup-status', `Setup failed: ${getErrorMessage(error)}`)
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

      const trackNamespace = parseTrackNamespace(form['publish-track-namespace'].value)

      console.info('[MediaPublisher] sending PUBLISH_NAMESPACE', { trackNamespace })
      await moqtClient.publishNamespace(trackNamespace, 'secret')
      setStatusText('publisher-announce-status', `Announced: ${trackNamespace.join('/')}`)
      console.info('[MediaPublisher] PUBLISH_NAMESPACE completed', { trackNamespace })
    } catch (error) {
      setStatusText('publisher-announce-status', `Announce failed: ${getErrorMessage(error)}`)
      console.error('[MediaPublisher] PUBLISH_NAMESPACE failed', error)
    }
  })
}

function sendSubgroupObjectButtonClickHandler(): void {
  const sendSubgroupObjectBtn = document.getElementById('sendSubgroupObjectBtn') as HTMLButtonElement
  sendSubgroupObjectBtn.addEventListener('click', async () => {
    console.log('clicked sendSubgroupObjectBtn')
    if (mediaStream == null) {
      setStatusText('publisher-send-status', 'Media stream is not ready')
      console.error('mediaStream is null')
      return
    }
    if (publishingStarted) {
      setStatusText('publisher-send-status', 'Publishing already started')
      return
    }
    let client: MOQTClient
    try {
      client = ensureClient()
    } catch (error) {
      setStatusText('publisher-send-status', `Publish failed: ${getErrorMessage(error)}`)
      console.error(error)
      return
    }
    publishingStarted = true
    setStatusText('publisher-send-status', 'Publishing started')

    videoEncoderWorker.onmessage = async (event: MessageEvent<VideoEncoderWorkerMessage>) => {
      const data = event.data
      if (data.type !== 'chunk') {
        return
      }
      const { chunk, metadata, captureTimestampMicros } = data
      const trackNamespace = parseTrackNamespace(form['publish-track-namespace'].value)
      const publisherPriority = Number(form['video-publisher-priority'].value)
      const trackAliases = getVideoTrackAliases(client, trackNamespace)
      if (!trackAliases.length) {
        return
      }
      if (!firstVideoChunkSent) {
        firstVideoChunkSent = true
        setStatusText('publisher-send-status', 'Streaming video chunks')
      }

      await sendVideoChunkViaMoqt({
        chunk,
        metadata,
        captureTimestampMicros,
        trackAliases,
        publisherPriority,
        client,
        transportState
      })
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
  })
}

function setupClientCallbacks(): void {
  moqtClient.setOnServerSetupHandler((serverSetup: any) => {
    setupCompleted = true
    console.log({ serverSetup })
    setStatusText('publisher-setup-status', 'Setup acknowledged')
  })

  moqtClient.setOnPublishNamespaceResponseHandler((responseMessage) => {
    console.log({ publishNamespaceResponseMessage: responseMessage })
  })

  moqtClient.setOnSubscribeResponseHandler((subscribeResponseMessage) => {
    console.log({ subscribeResponseMessage })
  })

  moqtClient.setOnIncomingSubscribeHandler(async ({ subscribe, isSuccess, code, respondOk, respondError }) => {
    console.log({ subscribeMessage: subscribe })
    const trackNamespace = parseTrackNamespace(form['publish-track-namespace'].value)
    const requestedNamespace = subscribe.trackNamespace ?? []
    const trackName = subscribe.trackName ?? ''
    const namespaceMatched = requestedNamespace.join('/') === trackNamespace.join('/')

    if (isSuccess) {
      if (!namespaceMatched) {
        setStatusText('publisher-send-status', `Rejected subscribe: unknown namespace ${requestedNamespace.join('/')}`)
        await respondError(404n, 'unknown namespace')
        return
      }
      if (isCatalogTrack(trackName)) {
        const trackAlias = await respondOk(0n)
        const client = ensureClient()
        await sendCatalog(client, trackAlias, trackNamespace)
        return
      }
      if (isVideoTrack(trackName)) {
        await respondOk(0n)
        activeSubscriberTracks.add(trackName)
        setStatusText(
          'publisher-send-status',
          `Subscriber ready: ${Array.from(activeSubscriberTracks).sort().join(', ')}`
        )
        return
      }
      setStatusText('publisher-send-status', `Rejected subscribe: unknown track ${trackName}`)
      await respondError(404n, 'unknown track')
      return
    }

    const reasonPhrase = `subscribe error: code=${code}`
    setStatusText('publisher-send-status', reasonPhrase)
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
    initializeStatuses()
    setStatusText('publisher-connection-status', 'Disconnected')
  })
}

setUpStartGetUserMediaButton()
setupClientCallbacks()
setupCloseButtonHandler()
initializeMediaExamplePage('publish-track-namespace')
initializeStatuses()

const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
connectBtn.addEventListener('click', async () => {
  const url = form.url.value

  setupCompleted = false
  await moqtClient.connect(url, { sendSetup: false })
  catalogAliases.clear()
  console.info('[MediaPublisher] connected', { url })
  activeSubscriberTracks.clear()
  publishingStarted = false
  firstVideoChunkSent = false
  setStatusText('publisher-connection-status', `Connected: ${url}`)
  setStatusText('publisher-send-status', 'Waiting for subscribers')
})

sendSetupButtonClickHandler()
sendPublishNamespaceButtonClickHandler()
sendSubgroupObjectButtonClickHandler()
