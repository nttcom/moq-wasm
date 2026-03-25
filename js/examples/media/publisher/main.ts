import { MoqtClientWrapper } from '@moqt/moqtClient'
import type { MOQTClient } from '../../../pkg/moqt_client_wasm'
import { AUTH_INFO } from './const'
import { sendVideoObjectMessage } from './sender'
import { getFormElement } from './utils'
import { MediaTransportState } from '../../../utils/media/transportState'
import { sendVideoChunkViaMoqt } from '../../../utils/media/videoTransport'
import { sendAudioChunkViaMoqt } from '../../../utils/media/audioTransport'
import { KEYFRAME_INTERVAL } from '../../../utils/media/constants'
import { MEDIA_AUDIO_PROFILES, MEDIA_CATALOG_TRACK_NAME, MEDIA_VIDEO_PROFILES, buildMediaCatalogJson } from '../catalog'

let mediaStream: MediaStream | null = null
const ENABLE_AUDIO_PUBLISH = false
const moqtClient = new MoqtClientWrapper()
const transportState = new MediaTransportState()
const catalogAliases = new Set<string>()
let setupCompleted = false

function ensureClient(): MOQTClient {
  const client = moqtClient.getRawClient()
  if (!client) {
    throw new Error('MOQT client not connected')
  }
  return client
}

function setUpStartGetUserMediaButton() {
  const startGetUserMediaBtn = document.getElementById('startGetUserMediaBtn') as HTMLButtonElement
  startGetUserMediaBtn.addEventListener('click', async () => {
    const constraints = {
      audio: true,
      video: {
        width: { exact: 1920 },
        height: { exact: 1080 }
        //   width: 3840,
        //   height: 2160,
      }
    }
    mediaStream = await navigator.mediaDevices.getUserMedia(constraints)
    const video = document.getElementById('video') as HTMLVideoElement
    video.srcObject = mediaStream
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

function parseTrackNamespace(raw: string): string[] {
  return raw
    .split('/')
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
}

function getForwardingPreference(form: HTMLFormElement): string {
  return (Array.from(form['forwarding-preference']) as HTMLInputElement[]).filter((elem) => elem.checked)[0].value
}

function isCatalogTrack(trackName: string): boolean {
  return trackName === MEDIA_CATALOG_TRACK_NAME
}

function isVideoTrack(trackName: string): boolean {
  return MEDIA_VIDEO_PROFILES.some((profile) => profile.trackName === trackName)
}

function isAudioTrack(trackName: string): boolean {
  return MEDIA_AUDIO_PROFILES.some((profile) => profile.trackName === trackName)
}

function getVideoTrackAliases(client: MOQTClient, trackNamespace: string[]): bigint[] {
  const aliases = new Set<string>()
  for (const profile of MEDIA_VIDEO_PROFILES) {
    for (const alias of client.getTrackSubscribers(trackNamespace, profile.trackName)) {
      aliases.add(alias.toString())
    }
  }
  return Array.from(aliases, (alias) => BigInt(alias))
}

function getAudioTrackAliases(client: MOQTClient, trackNamespace: string[], trackName: string): bigint[] {
  return Array.from(client.getTrackSubscribers(trackNamespace, trackName), (alias) => BigInt(alias))
}

async function sendCatalog(client: MOQTClient, trackAlias: bigint, trackNamespace: string[]): Promise<void> {
  const aliasKey = trackAlias.toString()
  if (catalogAliases.has(aliasKey)) {
    return
  }
  const payload = new TextEncoder().encode(buildMediaCatalogJson(trackNamespace))
  await client.sendSubgroupHeader(trackAlias, 0n, 0n, 0)
  await client.sendSubgroupObject(trackAlias, 0n, 0n, 0n, undefined, payload, undefined)
  catalogAliases.add(aliasKey)
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
      console.info('[MediaPublisher] CLIENT_SETUP sent', { maxSubscribeId: maxSubscribeId.toString() })
    } catch (error) {
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
      console.info('[MediaPublisher] PUBLISH_NAMESPACE completed', { trackNamespace })
    } catch (error) {
      console.error('[MediaPublisher] PUBLISH_NAMESPACE failed', error)
    }
  })
}

function sendSubgroupObjectButtonClickHandler(): void {
  const sendSubgroupObjectBtn = document.getElementById('sendSubgroupObjectBtn') as HTMLButtonElement
  sendSubgroupObjectBtn.addEventListener('click', async () => {
    console.log('clicked sendSubgroupObjectBtn')
    if (mediaStream == null) {
      console.error('mediaStream is null')
      return
    }
    let client: MOQTClient
    try {
      client = ensureClient()
    } catch (error) {
      console.error(error)
      return
    }

    videoEncoderWorker.onmessage = async (event: MessageEvent<VideoEncoderWorkerMessage>) => {
      const data = event.data
      if (data.type !== 'chunk') {
        return
      }
      const { chunk, metadata, captureTimestampMicros } = data
      console.debug('[MediaPublisher] video chunk', {
        byteLength: chunk.byteLength,
        type: chunk.type,
        timestamp: chunk.timestamp
      })
      const form = getFormElement()
      const trackNamespace = parseTrackNamespace(form['publish-track-namespace'].value)
      const publisherPriority = Number(form['video-publisher-priority'].value)
      const trackAliases = getVideoTrackAliases(client, trackNamespace)
      if (!trackAliases.length) {
        return
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
    const videoProcessor = new MediaStreamTrackProcessor({ track: videoTrack })
    const videoStream = videoProcessor.readable
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
  sendSetupButtonClickHandler()
  sendPublishNamespaceButtonClickHandler()
  sendSubgroupObjectButtonClickHandler()
}

function setupClientCallbacks(): void {
  moqtClient.setOnServerSetupHandler((serverSetup: any) => {
    setupCompleted = true
    console.log({ serverSetup })
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
        return
      }
      await respondError(404n, 'unknown track')
      return
    }

    const reasonPhrase = `subscribe error: code=${code}`
    await respondError(BigInt(code), reasonPhrase)
  })
}

function setupCloseButtonHandler(): void {
  const closeBtn = document.getElementById('closeBtn') as HTMLButtonElement
  closeBtn.addEventListener('click', async () => {
    await moqtClient.disconnect()
    catalogAliases.clear()
  })
}

setUpStartGetUserMediaButton()
setupClientCallbacks()
setupCloseButtonHandler()

const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
connectBtn.addEventListener('click', async () => {
  const form = getFormElement()
  const url = form.url.value

  setupCompleted = false
  await moqtClient.connect(url, { sendSetup: false })
  catalogAliases.clear()
  console.info('[MediaPublisher] connected', { url })
})

setupButtonClickHandler()
