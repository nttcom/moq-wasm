import { MoqtClientWrapper } from '@moqt/moqtClient'
import type { MOQTClient } from '../../../pkg/moqt_client_wasm'
import { AUTH_INFO, CMAF_FPS, CMAF_KEYFRAME_INTERVAL_FRAMES } from '../const'
import { getFormElement } from '../utils'
import { MediaTransportState } from '../../../utils/media/transportState'
import { MEDIA_VIDEO_PROFILES, MEDIA_CATALOG_TRACK_NAME, buildCmafCatalogJson } from '../catalog'
import { Output, NullTarget, Mp4OutputFormat, MediaStreamVideoTrackSource, MediaStreamAudioTrackSource } from 'mediabunny'

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
      audio: true,
      video: {
        width: { exact: 1920 },
        height: { exact: 1080 }
      }
    }
    mediaStream = await navigator.mediaDevices.getUserMedia(constraints)
    const video = document.getElementById('video') as HTMLVideoElement
    video.srcObject = mediaStream
  })
}

async function sendCmafObject(
  trackAlias: bigint,
  groupId: bigint,
  objectId: bigint,
  segment: Uint8Array,
  client: MOQTClient
): Promise<void> {
  // Prepend 8-byte timestamp (Date.now()) for E2E delay measurement
  const payload = new Uint8Array(8 + segment.byteLength)
  new DataView(payload.buffer).setBigUint64(0, BigInt(Date.now()))
  payload.set(segment, 8)
  await client.sendSubgroupStreamObject(trackAlias, groupId, 0n, objectId, undefined, payload, undefined)
}

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

    const keyFrameIntervalSec = CMAF_KEYFRAME_INTERVAL_FRAMES / CMAF_FPS

    // --- fMP4 fragment state ---
    // onFtyp/onMoov で init segment (ftyp + moov) を組み立て、
    // onMoof/onMdat で media segment (moof + mdat) を組み立てて MoQT で送信する。
    let initSegment: Uint8Array | null = null
    const initParts: Uint8Array[] = []
    let lastMoof: Uint8Array | null = null

    // --- mediabunny Output (fragmented fMP4) ---
    const output = new Output({
      target: new NullTarget(),
      format: new Mp4OutputFormat({
        fastStart: 'fragmented',
        // キーフレームが minimumFragmentDuration より僅かに早く届くと
        // 2 GoP がまとまってしまうため、余裕を持たせる
        minimumFragmentDuration: keyFrameIntervalSec - 0.5,

        onFtyp: (data) => {
          initParts.push(new Uint8Array(data))
        },
        onMoov: (data) => {
          initParts.push(new Uint8Array(data))
          const totalLen = initParts.reduce((sum, part) => sum + part.byteLength, 0)
          initSegment = new Uint8Array(totalLen)
          let offset = 0
          for (const part of initParts) {
            initSegment.set(part, offset)
            offset += part.byteLength
          }
          console.info('[CmafPublisher] init segment ready', { byteLength: initSegment.byteLength })
        },
        onMoof: (data) => {
          lastMoof = new Uint8Array(data)
        },

        // --- MoQT 送信 ---
        // フラグメント完成時 (moof + mdat) に呼ばれる。
        // 1 フラグメント = 1 GoP = 1 MoQT Group として送信する。
        // Group 内の Object 構成: 0 = init segment, 1 = media segment
        onMdat: async (data) => {
          if (!lastMoof || !initSegment) {
            return
          }

          const mediaSegment = new Uint8Array(lastMoof.byteLength + data.byteLength)
          mediaSegment.set(lastMoof, 0)
          mediaSegment.set(new Uint8Array(data), lastMoof.byteLength)

          const form = getFormElement()
          const trackNamespace = parseTrackNamespace(form['announce-track-namespace'].value)
          const publisherPriority = Number(form['video-publisher-priority'].value)
          const trackAliases = getVideoTrackAliases(client, trackNamespace)
          if (!trackAliases.length) {
            return
          }

          transportState.advanceVideoGroup()

          for (const alias of trackAliases) {
            await client.sendSubgroupStreamHeaderMessage(alias, transportState.getVideoGroupId(), 0n, publisherPriority)
            await sendCmafObject(alias, transportState.getVideoGroupId(), 0n, initSegment, client)
            await sendCmafObject(alias, transportState.getVideoGroupId(), 1n, mediaSegment, client)
          }
        },
      }),
    })

    // --- メディアソース (gUM → mediabunny) ---
    const [videoTrack] = mediaStream.getVideoTracks()
    const videoSource = new MediaStreamVideoTrackSource(videoTrack, {
      codec: 'avc',
      bitrate: 4_000_000,
      latencyMode: 'realtime',
      keyFrameInterval: keyFrameIntervalSec,
    })
    output.addVideoTrack(videoSource, { frameRate: CMAF_FPS })
    videoSource.errorPromise.catch((error) => {
      console.error('[CmafPublisher] video source error', error)
    })

    const [audioTrack] = mediaStream.getAudioTracks()
    if (audioTrack) {
      const audioSource = new MediaStreamAudioTrackSource(audioTrack, {
        codec: 'aac',
        bitrate: 128_000,
      })
      output.addAudioTrack(audioSource)
      audioSource.errorPromise.catch((error) => {
        console.error('[CmafPublisher] audio source error', error)
      })
    }

    await output.start()
    console.info('[CmafPublisher] output started')
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
