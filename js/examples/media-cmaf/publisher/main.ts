import { MoqtClientWrapper } from '@moqt/moqtClient'
import type { MOQTClient } from '../../../pkg/moqt_client_wasm'
import { AUTH_INFO } from '../const'
import { sendCmafVideoObject } from './sender'
import { getFormElement } from '../utils'
import { CmafVideoMuxer } from './cmafMuxer'
import { MediaTransportState } from '../../../utils/media/transportState'
import { KEYFRAME_INTERVAL } from '../../../utils/media/constants'
import {
  MEDIA_VIDEO_PROFILES,
  MEDIA_CATALOG_TRACK_NAME,
} from '../catalog'
import { buildCmafCatalogJson } from '../catalog'

let mediaStream: MediaStream | null = null
const moqtClient = new MoqtClientWrapper()
const transportState = new MediaTransportState()
const catalogAliases = new Set<string>()

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
      audio: false, // video only for now
      video: {
        width: { exact: 1920 },
        height: { exact: 1080 },
      },
    }
    mediaStream = await navigator.mediaDevices.getUserMedia(constraints)
    const video = document.getElementById('video') as HTMLVideoElement
    video.srcObject = mediaStream
  })
}

// CMAF video encoder worker (uses avc format instead of annexb)
const videoEncoderWorker = new Worker(
  new URL('./videoEncoderCmaf.ts', import.meta.url),
  { type: 'module' }
)

type VideoEncoderWorkerMessage =
  | {
      type: 'chunk'
      chunk: EncodedVideoChunk
      metadata: EncodedVideoChunkMetadata | undefined
      captureTimestampMicros?: number
    }
  | { type: 'bitrate'; kbps: number }

function parseTrackNamespace(raw: string): string[] {
  return raw
    .split('/')
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
}

function isCatalogTrack(trackName: string): boolean {
  return trackName === MEDIA_CATALOG_TRACK_NAME
}

function isVideoTrack(trackName: string): boolean {
  return MEDIA_VIDEO_PROFILES.some((profile) => profile.trackName === trackName)
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

async function sendCatalog(client: MOQTClient, trackAlias: bigint, trackNamespace: string[]): Promise<void> {
  const aliasKey = trackAlias.toString()
  if (catalogAliases.has(aliasKey)) {
    return
  }
  const payload = new TextEncoder().encode(buildCmafCatalogJson(trackNamespace))
  await client.sendSubgroupStreamHeaderMessage(trackAlias, 0n, 0n, 0)
  await client.sendSubgroupStreamObject(trackAlias, 0n, 0n, 0n, undefined, payload, undefined)
  catalogAliases.add(aliasKey)
  console.info('[CmafPublisher] sent catalog', { trackAlias: aliasKey, trackNamespace })
}

function sendSetupButtonClickHandler(): void {
  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement
  sendSetupBtn.addEventListener('click', async () => {
    const form = getFormElement()
    const versions = new BigUint64Array('0xff00000A'.split(',').map(BigInt))
    const maxSubscribeId = BigInt(form['max-subscribe-id'].value)
    await moqtClient.sendSetupMessage(versions, maxSubscribeId)
  })
}

function sendAnnounceButtonClickHandler(): void {
  const sendAnnounceBtn = document.getElementById('sendAnnounceBtn') as HTMLButtonElement
  sendAnnounceBtn.addEventListener('click', async () => {
    const form = getFormElement()
    const trackNamespace = parseTrackNamespace(form['announce-track-namespace'].value)
    await moqtClient.announce(trackNamespace, AUTH_INFO)
  })
}

function sendSubgroupObjectButtonClickHandler(): void {
  const sendSubgroupObjectBtn = document.getElementById('sendSubgroupObjectBtn') as HTMLButtonElement
  sendSubgroupObjectBtn.addEventListener('click', async () => {
    console.log('clicked sendSubgroupObjectBtn (CMAF)')
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

    // Store init segment — sent as object 0 of each new group
    let initSegment: Uint8Array | null = null

    // Create CMAF muxer with callback that sends via MoQT
    const cmafMuxer = new CmafVideoMuxer(async (type, segment, isKeyframe) => {
      // Save init segment for later, don't send it directly
      if (type === 'init') {
        initSegment = new Uint8Array(segment)
        console.info('[CmafPublisher] init segment stored', { byteLength: initSegment.byteLength })
        return
      }

      const form = getFormElement()
      const trackNamespace = parseTrackNamespace(form['announce-track-namespace'].value)
      const publisherPriority = Number(form['video-publisher-priority'].value)
      const trackAliases = getVideoTrackAliases(client, trackNamespace)
      if (!trackAliases.length) {
        return
      }

      // Wait for keyframe to start a new group (groupId == -1n means not started)
      if (!isKeyframe && transportState.getVideoGroupId() < 0n) {
        return
      }

      if (isKeyframe) {
        transportState.advanceVideoGroup()
      }

      for (const alias of trackAliases) {
        if (!transportState.hasVideoHeaderSent(alias, 0)) {
          await client.sendSubgroupStreamHeaderMessage(
            alias,
            transportState.getVideoGroupId(),
            0n,
            publisherPriority
          )
          transportState.markVideoHeaderSent(alias, 0)

          // Send init segment as object 0
          if (initSegment) {
            await sendCmafVideoObject(
              alias,
              transportState.getVideoGroupId(),
              0n,
              transportState.getVideoObjectId(),
              initSegment,
              client
            )
            transportState.incrementVideoObject()
          }
        }

        await sendCmafVideoObject(
          alias,
          transportState.getVideoGroupId(),
          0n,
          transportState.getVideoObjectId(),
          new Uint8Array(segment),
          client
        )
      }

      transportState.incrementVideoObject()
    })

    // Wire up encoder worker output to CMAF muxer
    videoEncoderWorker.onmessage = async (event: MessageEvent<VideoEncoderWorkerMessage>) => {
      const data = event.data
      if (data.type !== 'chunk') {
        return
      }
      const { chunk, metadata } = data
      console.debug('[CmafPublisher] video chunk', {
        byteLength: chunk.byteLength,
        type: chunk.type,
        timestamp: chunk.timestamp,
      })

      // Feed to CMAF muxer - the muxer's callback handles sending
      cmafMuxer.addVideoChunk(chunk, metadata)
    }

    // Start encoding
    const [videoTrack] = mediaStream.getVideoTracks()
    const videoProcessor = new MediaStreamTrackProcessor({ track: videoTrack })
    const videoStream = videoProcessor.readable
    videoEncoderWorker.postMessage({
      type: 'keyframeInterval',
      keyframeInterval: KEYFRAME_INTERVAL,
    })
    videoEncoderWorker.postMessage({ type: 'videoStream', videoStream }, [videoStream])
  })
}

function setupButtonClickHandler(): void {
  sendSetupButtonClickHandler()
  sendAnnounceButtonClickHandler()
  sendSubgroupObjectButtonClickHandler()
}

function setupClientCallbacks(): void {
  moqtClient.setOnServerSetupHandler((serverSetup: any) => {
    console.log({ serverSetup })
  })

  moqtClient.setOnAnnounceHandler(async (announceMessage) => {
    console.log({ announceMessage })
    const client = ensureClient()
    await client.sendAnnounceOkMessage(announceMessage.trackNamespace)
  })

  moqtClient.setOnSubscribeResponseHandler((announceResponseMessage) => {
    console.log({ announceResponseMessage })
  })

  moqtClient.setOnIncomingSubscribeHandler(async ({ subscribe, isSuccess, code, respondOk, respondError }) => {
    console.log({ subscribeMessage: subscribe })
    const form = getFormElement()
    const trackNamespace = parseTrackNamespace(form['announce-track-namespace'].value)
    const requestedNamespace = subscribe.trackNamespace ?? []
    const trackName = subscribe.trackName ?? ''
    const namespaceMatched = requestedNamespace.join('/') === trackNamespace.join('/')

    if (isSuccess) {
      if (!namespaceMatched) {
        await respondError(404n, 'unknown namespace')
        return
      }
      if (isCatalogTrack(trackName)) {
        await respondOk(0n, AUTH_INFO, 'subgroup')
        const client = ensureClient()
        await sendCatalog(client, BigInt(subscribe.trackAlias), trackNamespace)
        return
      }
      if (isVideoTrack(trackName)) {
        await respondOk(0n, AUTH_INFO, 'subgroup')
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

  await moqtClient.connect(url, { sendSetup: false })
  catalogAliases.clear()
  setupButtonClickHandler()
})
