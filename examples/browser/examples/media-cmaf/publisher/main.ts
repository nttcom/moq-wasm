import { MoqtClientWrapper } from '@moqt/moqtClient'
import type { MOQTClient } from '../../../pkg/moqt'
import { MediaTransportState } from '../../../utils/media/transportState'
import { MEDIA_VIDEO_PROFILES, MEDIA_AUDIO_PROFILES } from '../catalog'
import { buildMediaCatalogJson } from '../../media/catalog'
import {
  Output,
  NullTarget,
  Mp4OutputFormat,
  MediaStreamVideoTrackSource,
  MediaStreamAudioTrackSource
} from 'mediabunny'
import { getTrackAliases, serveSubscribes } from '../../media/serveSubscribes'
import { setUpVisualizerToggle } from '../../media/visualizerToggle'
import { element, parseTrackNamespace, initializeMediaExamplePage } from '../../../utils/page'

let mediaStream: MediaStream | null = null
const moqtClient = new MoqtClientWrapper()
const form = element<HTMLFormElement>('form')
const transportState = new MediaTransportState()
const videoTrackNames = MEDIA_VIDEO_PROFILES.map((profile) => profile.trackName)
const audioTrackNames = MEDIA_AUDIO_PROFILES.map((profile) => profile.trackName)

type MoqSendVisualizationState = {
  groupId: bigint | null
  objectId: bigint | null
}

const moqSendVisualization: Record<'video' | 'audio', MoqSendVisualizationState> = {
  video: { groupId: null, objectId: null },
  audio: { groupId: null, objectId: null }
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
  if (isMoqSendVisualizerVisible()) {
    renderMoqSendVisualization()
  }
}

const resetMoqSendVisualizationState = (): void => {
  moqSendVisualization.video = { groupId: null, objectId: null }
  moqSendVisualization.audio = { groupId: null, objectId: null }
}

const renderMoqSendVisualization = (): void => {
  for (const kind of ['video', 'audio'] as const) {
    const { groupId, objectId } = moqSendVisualization[kind]
    element(`send-viz-${kind}-group-id`).textContent = groupId?.toString() ?? '-'
    element(`send-viz-${kind}-object-id`).textContent = objectId?.toString() ?? '-'
  }
}

const isMoqSendVisualizerVisible = setUpVisualizerToggle(
  'toggleMoqSendVisualizerBtn',
  'moqSendVisualizerPanel',
  'MoQ送信可視化',
  renderMoqSendVisualization
)

const sendSetupButtonClickHandler = (): void => {
  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement
  sendSetupBtn.addEventListener('click', async () => {
    const versions = new BigUint64Array([0xff00000en])
    const maxSubscribeId = BigInt(form['max-subscribe-id'].value)
    await moqtClient.sendClientSetup(versions, maxSubscribeId)
  })
}

const sendPublishNamespaceButtonClickHandler = (): void => {
  const sendPublishNamespaceBtn = document.getElementById('sendPublishNamespaceBtn') as HTMLButtonElement
  sendPublishNamespaceBtn.addEventListener('click', async () => {
    const trackNamespace = parseTrackNamespace(form['publish-track-namespace'].value)
    await moqtClient.publishNamespace(trackNamespace, 'secret')
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
      client = moqtClient.requireConnectedClient()
    } catch (error) {
      console.error(error)
      return
    }

    const keyFrameIntervalSec = Number(form['keyframe-interval'].value)

    const getTrackNamespace = () => parseTrackNamespace(form['publish-track-namespace'].value)

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
          () => getTrackAliases(client, getTrackNamespace(), videoTrackNames),
          () => transportState.video.advanceGroup(),
          () => transportState.video.groupId
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
          () => getTrackAliases(client, getTrackNamespace(), audioTrackNames),
          () => transportState.audio.advanceGroup(),
          () => transportState.audio.groupId
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
    videoOutput.addVideoTrack(videoSource, { frameRate: 30 })
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

const setupClientCallbacks = (): void => {
  moqtClient.setOnServerSetupHandler((serverSetup: any) => {
    console.log({ serverSetup })
  })

  moqtClient.setOnSubscribeResponseHandler((subscribeResponseMessage) => {
    console.log({ subscribeResponseMessage })
  })

  serveSubscribes(moqtClient, {
    trackNamespace: () => parseTrackNamespace(form['publish-track-namespace'].value),
    catalogJson: (trackNamespace) =>
      buildMediaCatalogJson(trackNamespace, 'cmaf', MEDIA_VIDEO_PROFILES, MEDIA_AUDIO_PROFILES),
    mediaTrackNames: [...videoTrackNames, ...audioTrackNames],
    onStatus: (text) => console.info('[CmafPublisher]', text)
  })
}

const setupCloseButtonHandler = (): void => {
  const closeBtn = document.getElementById('closeBtn') as HTMLButtonElement
  closeBtn.addEventListener('click', async () => {
    await moqtClient.disconnect()
    resetMoqSendVisualizationState()
    if (isMoqSendVisualizerVisible()) {
      renderMoqSendVisualization()
    }
  })
}

initializeMediaExamplePage('publish-track-namespace')
setUpStartGetUserMediaButton()
setupClientCallbacks()
setupCloseButtonHandler()

const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
connectBtn.addEventListener('click', async () => {
  const url = form.url.value

  await moqtClient.connect(url, { sendSetup: false })
  resetMoqSendVisualizationState()
  if (isMoqSendVisualizerVisible()) {
    renderMoqSendVisualization()
  }
})

sendSetupButtonClickHandler()
sendPublishNamespaceButtonClickHandler()
sendSubgroupObjectButtonClickHandler()
