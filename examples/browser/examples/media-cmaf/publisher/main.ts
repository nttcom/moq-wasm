import { MoqSession, type TrackWriter } from 'moq-wasm'
import { CATALOG_TRACK_NAME } from 'moq-wasm/media/catalog'
import { MEDIA_VIDEO_PROFILES, MEDIA_AUDIO_PROFILES } from '../catalog'
import { buildMediaCatalogJson } from '../../media/catalog'
import {
  Output,
  NullTarget,
  Mp4OutputFormat,
  MediaStreamVideoTrackSource,
  MediaStreamAudioTrackSource
} from 'mediabunny'
import { setUpVisualizerToggle } from '../../media/visualizerToggle'
import { element, parseTrackNamespace, initializeMediaExamplePage } from '../../../utils/page'

let mediaStream: MediaStream | null = null
let session: MoqSession | undefined
const writers: Record<'video' | 'audio', TrackWriter[]> = { video: [], audio: [] }
const form = element<HTMLFormElement>('form')
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

const sendPublishNamespaceButtonClickHandler = (): void => {
  const sendPublishNamespaceBtn = document.getElementById('sendPublishNamespaceBtn') as HTMLButtonElement
  sendPublishNamespaceBtn.addEventListener('click', async () => {
    const connected = session
    if (!connected) {
      console.error('[CmafPublisher] not connected')
      return
    }
    const trackNamespace = parseTrackNamespace(form['publish-track-namespace'].value)
    await connected.announce(trackNamespace)
    writers.video = await Promise.all(videoTrackNames.map((name) => connected.publishTrack(trackNamespace, name)))
    writers.audio = await Promise.all(audioTrackNames.map((name) => connected.publishTrack(trackNamespace, name)))
    // Keeping only the newest object leaves the catalog for each joining FETCH.
    const catalog = await connected.publishTrack(trackNamespace, CATALOG_TRACK_NAME, { retentionMs: 0 })
    await catalog.writeGroup(
      new TextEncoder().encode(
        buildMediaCatalogJson(trackNamespace, 'cmaf', MEDIA_VIDEO_PROFILES, MEDIA_AUDIO_PROFILES)
      )
    )
  })
}

const sendSubgroupObjectButtonClickHandler = (): void => {
  const sendSubgroupObjectBtn = document.getElementById('sendSubgroupObjectBtn') as HTMLButtonElement
  sendSubgroupObjectBtn.addEventListener('click', async () => {
    if (mediaStream == null) {
      console.error('mediaStream is null')
      return
    }
    if (!writers.video.length) {
      console.error('[CmafPublisher] send PUBLISH_NAMESPACE first')
      return
    }

    const keyFrameIntervalSec = Number(form['keyframe-interval'].value)

    // --- fMP4 fragment コールバック生成 ---
    // mediabunny はキーフレーム境界でフラグメントを切るため、
    // 各フラグメント = 1 MoQ Group（Object 0 = init, Object 1 = moof+mdat）
    const buildMp4Callbacks = (type: 'video' | 'audio') => {
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

          for (const writer of writers[type]) {
            const groupId = await writer.write(initSegment, true, undefined)
            await writer.write(fragment, false, undefined)
            if (groupId !== undefined) {
              setMoqSendVisualizationState(type, groupId, 1n)
            }
          }
        }
      }
    }

    // --- 映像用 Output ---
    const videoOutput = new Output({
      target: new NullTarget(),
      format: new Mp4OutputFormat({
        fastStart: 'fragmented',
        ...buildMp4Callbacks('video')
      })
    })

    // --- 音声用 Output ---
    const audioOutput = new Output({
      target: new NullTarget(),
      format: new Mp4OutputFormat({
        fastStart: 'fragmented',
        ...buildMp4Callbacks('audio')
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

const setupCloseButtonHandler = (): void => {
  const closeBtn = document.getElementById('closeBtn') as HTMLButtonElement
  closeBtn.addEventListener('click', async () => {
    session?.close()
    session = undefined
    writers.video = []
    writers.audio = []
    resetMoqSendVisualizationState()
    if (isMoqSendVisualizerVisible()) {
      renderMoqSendVisualization()
    }
  })
}

initializeMediaExamplePage('publish-track-namespace')
setUpStartGetUserMediaButton()
setupCloseButtonHandler()

const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
connectBtn.addEventListener('click', async () => {
  const url = form.url.value

  session = await MoqSession.connect(url)
  resetMoqSendVisualizationState()
  if (isMoqSendVisualizerVisible()) {
    renderMoqSendVisualization()
  }
})

sendPublishNamespaceButtonClickHandler()
sendSubgroupObjectButtonClickHandler()
