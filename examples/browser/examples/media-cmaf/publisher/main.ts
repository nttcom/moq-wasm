import { MoqtClientWrapper } from '@moqt/moqtClient'
import type { MOQTClient } from '../../../pkg/moqt_client_wasm'
import { AUTH_INFO, CMAF_FPS } from '../const'
import { getFormElement } from '../utils'
import { MediaTransportState } from '../../../utils/media/transportState'
import { MEDIA_VIDEO_PROFILES, MEDIA_AUDIO_PROFILES, MEDIA_CATALOG_TRACK_NAME, buildCmafCatalogJson } from '../catalog'
import {
  Output,
  NullTarget,
  Mp4OutputFormat,
  MediaStreamVideoTrackSource,
  MediaStreamAudioTrackSource
} from 'mediabunny'

let mediaStream: MediaStream | null = null
const moqtClient = new MoqtClientWrapper()
const transportState = new MediaTransportState()
const catalogAliases = new Set<string>()

type MoqSendVisualizationState = {
  groupId: bigint | null
  objectId: bigint | null
}

const moqSendVisualization: Record<'video' | 'audio', MoqSendVisualizationState> = {
  video: { groupId: null, objectId: null },
  audio: { groupId: null, objectId: null }
}

let isMoqSendVisualizerVisible = false

const ensureClient = (): MOQTClient => {
  const client = moqtClient.getRawClient()
  if (!client) {
    throw new Error('MOQT client not connected')
  }
  return client
}

const setUpStartGetUserMediaButton = () => {
  const startGetUserMediaBtn = document.getElementById('startGetUserMediaBtn') as HTMLButtonElement
  startGetUserMediaBtn.addEventListener('click', async () => {
    const constraints = {
      audio: true,
      video: {
        width: { exact: 1280 },
        height: { exact: 720 }
      }
    }
    mediaStream = await navigator.mediaDevices.getUserMedia(constraints)
    const video = document.getElementById('video') as HTMLVideoElement
    video.srcObject = mediaStream
  })
}

const sendMoqObject = async (
  trackAlias: bigint,
  groupId: bigint,
  objectId: bigint,
  data: Uint8Array,
  client: MOQTClient,
  objectStatus?: number
): Promise<void> => {
  await client.sendSubgroupObject(trackAlias, groupId, 0n, objectId, objectStatus, data, undefined)
}

const setMoqSendVisualizationState = (type: 'video' | 'audio', groupId: bigint, objectId: bigint): void => {
  moqSendVisualization[type] = { groupId, objectId }
  if (isMoqSendVisualizerVisible) {
    renderMoqSendVisualization()
  }
}

const resetMoqSendVisualizationState = (): void => {
  moqSendVisualization.video = { groupId: null, objectId: null }
  moqSendVisualization.audio = { groupId: null, objectId: null }
}

const renderMoqSendVisualization = (): void => {
  const setText = (id: string, text: string) => {
    const el = document.getElementById(id)
    if (el) {
      el.textContent = text
    }
  }
  const formatBigInt = (value: bigint | null): string => (value === null ? '-' : value.toString())
  setText('send-viz-video-group-id', formatBigInt(moqSendVisualization.video.groupId))
  setText('send-viz-video-object-id', formatBigInt(moqSendVisualization.video.objectId))
  setText('send-viz-audio-group-id', formatBigInt(moqSendVisualization.audio.groupId))
  setText('send-viz-audio-object-id', formatBigInt(moqSendVisualization.audio.objectId))
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

const syncMoqSendVisualizerButton = (toggleBtn: HTMLButtonElement): void => {
  toggleBtn.innerHTML = getVisualizerEyeSvg(isMoqSendVisualizerVisible)
  toggleBtn.classList.toggle('active', isMoqSendVisualizerVisible)
  toggleBtn.setAttribute('aria-pressed', isMoqSendVisualizerVisible ? 'true' : 'false')
  const label = isMoqSendVisualizerVisible ? 'MoQ送信可視化を非表示' : 'MoQ送信可視化を表示'
  toggleBtn.setAttribute('aria-label', label)
  toggleBtn.setAttribute('title', label)
}

const setupMoqSendVisualizationButton = (): void => {
  const toggleBtn = document.getElementById('toggleMoqSendVisualizerBtn') as HTMLButtonElement | null
  const panel = document.getElementById('moqSendVisualizerPanel')
  if (!toggleBtn || !panel) {
    return
  }
  syncMoqSendVisualizerButton(toggleBtn)
  toggleBtn.addEventListener('click', () => {
    isMoqSendVisualizerVisible = !isMoqSendVisualizerVisible
    panel.classList.toggle('is-visible', isMoqSendVisualizerVisible)
    syncMoqSendVisualizerButton(toggleBtn)
    if (isMoqSendVisualizerVisible) {
      renderMoqSendVisualization()
    }
  })
}

const parseTrackNamespace = (raw: string): string[] => {
  return raw
    .split('/')
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
}

const isCatalogTrack = (trackName: string): boolean => {
  return trackName === MEDIA_CATALOG_TRACK_NAME
}

const isVideoTrack = (trackName: string): boolean => {
  return MEDIA_VIDEO_PROFILES.some((profile) => profile.trackName === trackName)
}

const isAudioTrack = (trackName: string): boolean => {
  return MEDIA_AUDIO_PROFILES.some((profile) => profile.trackName === trackName)
}

const getVideoTrackAliases = (client: MOQTClient, trackNamespace: string[]): bigint[] => {
  const aliases = new Set<string>()
  for (const profile of MEDIA_VIDEO_PROFILES) {
    for (const alias of client.getTrackSubscribers(trackNamespace, profile.trackName)) {
      aliases.add(alias.toString())
    }
  }
  return Array.from(aliases, (alias) => BigInt(alias))
}

const getAudioTrackAliases = (client: MOQTClient, trackNamespace: string[]): bigint[] => {
  const aliases = new Set<string>()
  for (const profile of MEDIA_AUDIO_PROFILES) {
    for (const alias of client.getTrackSubscribers(trackNamespace, profile.trackName)) {
      aliases.add(alias.toString())
    }
  }
  return Array.from(aliases, (alias) => BigInt(alias))
}

const sendCatalog = async (client: MOQTClient, trackAlias: bigint, trackNamespace: string[]): Promise<void> => {
  const aliasKey = trackAlias.toString()
  if (catalogAliases.has(aliasKey)) {
    return
  }
  const payload = new TextEncoder().encode(buildCmafCatalogJson(trackNamespace))
  await client.sendSubgroupHeader(trackAlias, 0n, 0n, 0)
  await client.sendSubgroupObject(trackAlias, 0n, 0n, 0n, undefined, payload, undefined)
  catalogAliases.add(aliasKey)
  console.info('[CmafPublisher] sent catalog', { trackAlias: aliasKey, trackNamespace })
}

const sendSetupButtonClickHandler = (): void => {
  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement
  sendSetupBtn.addEventListener('click', async () => {
    const form = getFormElement()
    const versions = new BigUint64Array('0xff00000E'.split(',').map(BigInt))
    const maxSubscribeId = BigInt(form['max-subscribe-id'].value)
    await moqtClient.sendClientSetup(versions, maxSubscribeId)
  })
}

const sendPublishNamespaceButtonClickHandler = (): void => {
  const sendPublishNamespaceBtn = document.getElementById('sendPublishNamespaceBtn') as HTMLButtonElement
  sendPublishNamespaceBtn.addEventListener('click', async () => {
    const form = getFormElement()
    const trackNamespace = parseTrackNamespace(form['publish-track-namespace'].value)
    await moqtClient.publishNamespace(trackNamespace, AUTH_INFO)
  })
}

const sendSubgroupObjectButtonClickHandler = (): void => {
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

    const form = getFormElement()
    const keyFrameIntervalSec = Number(form['keyframe-interval'].value)

    const getTrackNamespace = () => {
      const form = getFormElement()
      return parseTrackNamespace(form['publish-track-namespace'].value)
    }

    // --- fMP4 fragment コールバック生成 ---
    // mediabunny はキーフレーム境界でフラグメントを切るため、
    // 各フラグメント = 1 MoQ Group（Object 0 = init, Object 1 = moof+mdat）
    const buildMp4Callbacks = (
      type: 'video' | 'audio',
      getAliases: () => bigint[],
      advanceGroup: () => void,
      getGroupId: () => bigint
    ) => {
      let initSegment: Uint8Array | null = null
      const initParts: Uint8Array[] = []
      let lastMoof: Uint8Array | null = null

      return {
        onFtyp: (data: Uint8Array, _position: number) => {
          initParts.push(new Uint8Array(data))
        },
        onMoov: (data: Uint8Array, _position: number) => {
          initParts.push(new Uint8Array(data))
          const totalLen = initParts.reduce((sum, part) => sum + part.byteLength, 0)
          initSegment = new Uint8Array(totalLen)
          let offset = 0
          for (const part of initParts) {
            initSegment.set(part, offset)
            offset += part.byteLength
          }
          console.info(`[CmafPublisher] ${type} init segment ready`, { byteLength: initSegment.byteLength })
        },
        onMoof: (data: Uint8Array, _position: number) => {
          lastMoof = new Uint8Array(data)
        },
        onMdat: async (data: Uint8Array, _position: number) => {
          if (!lastMoof || !initSegment) {
            return
          }

          const fragment = new Uint8Array(lastMoof.byteLength + data.byteLength)
          fragment.set(lastMoof, 0)
          fragment.set(data, lastMoof.byteLength)

          const form = getFormElement()
          const publisherPriority = Number(form['video-publisher-priority'].value)
          const trackAliases = getAliases()
          if (!trackAliases.length) {
            return
          }

          advanceGroup()
          const groupId = getGroupId()
          for (const alias of trackAliases) {
            await client.sendSubgroupHeader(alias, groupId, 0n, publisherPriority)
            setMoqSendVisualizationState(type, groupId, 0n)
            await sendMoqObject(alias, groupId, 0n, initSegment, client)
            setMoqSendVisualizationState(type, groupId, 1n)
            await sendMoqObject(alias, groupId, 1n, fragment, client)
            // Explicitly terminate each group stream so the stream writer can be closed.
            setMoqSendVisualizationState(type, groupId, 2n)
            await sendMoqObject(alias, groupId, 2n, new Uint8Array(0), client, 3)
          }
        }
      }
    }

    // --- 映像用 Output ---
    const videoOutput = new Output({
      target: new NullTarget(),
      format: new Mp4OutputFormat({
        fastStart: 'fragmented',
        ...buildMp4Callbacks(
          'video',
          () => getVideoTrackAliases(client, getTrackNamespace()),
          () => transportState.advanceVideoGroup(),
          () => transportState.getVideoGroupId()
        )
      })
    })

    // --- 音声用 Output ---
    const audioOutput = new Output({
      target: new NullTarget(),
      format: new Mp4OutputFormat({
        fastStart: 'fragmented',
        ...buildMp4Callbacks(
          'audio',
          () => getAudioTrackAliases(client, getTrackNamespace()),
          () => transportState.advanceAudioGroup(),
          () => transportState.getAudioGroupId()
        )
      })
    })

    // --- メディアソース (gUM → mediabunny) ---
    const [videoTrack] = mediaStream.getVideoTracks()
    const videoSource = new MediaStreamVideoTrackSource(videoTrack, {
      codec: 'avc',
      bitrate: 500_000,
      latencyMode: 'realtime',
      keyFrameInterval: keyFrameIntervalSec
    })
    videoOutput.addVideoTrack(videoSource, { frameRate: CMAF_FPS })
    videoSource.errorPromise.catch((error) => {
      console.error('[CmafPublisher] video source error', error)
    })

    const [audioTrack] = mediaStream.getAudioTracks()
    if (audioTrack) {
      const audioSource = new MediaStreamAudioTrackSource(audioTrack, {
        codec: 'aac',
        bitrate: 128_000
      })
      audioOutput.addAudioTrack(audioSource)
      audioSource.errorPromise.catch((error) => {
        console.error('[CmafPublisher] audio source error', error)
      })
    }

    await Promise.all([videoOutput.start(), audioOutput.start()])
    console.info('[CmafPublisher] video/audio outputs started')
  })
}

const setupButtonClickHandler = (): void => {
  sendSetupButtonClickHandler()
  sendPublishNamespaceButtonClickHandler()
  sendSubgroupObjectButtonClickHandler()
}

const setupClientCallbacks = (): void => {
  moqtClient.setOnServerSetupHandler((serverSetup: any) => {
    console.log({ serverSetup })
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
      if (isVideoTrack(trackName)) {
        await respondOk(0n)
        return
      }
      if (isAudioTrack(trackName)) {
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

const setupCloseButtonHandler = (): void => {
  const closeBtn = document.getElementById('closeBtn') as HTMLButtonElement
  closeBtn.addEventListener('click', async () => {
    await moqtClient.disconnect()
    catalogAliases.clear()
    resetMoqSendVisualizationState()
    if (isMoqSendVisualizerVisible) {
      renderMoqSendVisualization()
    }
  })
}

setUpStartGetUserMediaButton()
setupMoqSendVisualizationButton()
setupClientCallbacks()
setupCloseButtonHandler()

const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
connectBtn.addEventListener('click', async () => {
  const form = getFormElement()
  const url = form.url.value

  await moqtClient.connect(url, { sendSetup: false })
  catalogAliases.clear()
  resetMoqSendVisualizationState()
  if (isMoqSendVisualizerVisible) {
    renderMoqSendVisualization()
  }
  setupButtonClickHandler()
})
