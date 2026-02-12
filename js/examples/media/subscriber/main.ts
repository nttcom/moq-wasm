import { MoqtClientWrapper } from '@moqt/moqtClient'
import { AUTH_INFO } from './const'
import { getFormElement } from './utils'

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

type LocHeaderSummary = {
  present: boolean
  extensionCount: number
  hasCaptureTimestamp: boolean
  hasVideoConfig: boolean
  hasVideoFrameMarking: boolean
  hasAudioLevel: boolean
}

function summarizeLocHeader(locHeader: any): LocHeaderSummary {
  const extensions = Array.isArray(locHeader?.extensions) ? locHeader.extensions : []
  const has = (type: string) => extensions.some((ext: any) => ext?.type === type)
  return {
    present: Boolean(locHeader),
    extensionCount: extensions.length,
    hasCaptureTimestamp: has('captureTimestamp'),
    hasVideoConfig: has('videoConfig'),
    hasVideoFrameMarking: has('videoFrameMarking'),
    hasAudioLevel: has('audioLevel')
  }
}

function toBigUint64Array(value: string): BigUint64Array {
  const values = value
    .split(',')
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
    .map((part) => BigInt(part))
  return new BigUint64Array(values)
}

function sendSetupButtonClickHandler() {
  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement
  sendSetupBtn.addEventListener('click', async () => {
    const form = getFormElement()

    const versions = toBigUint64Array('0xff00000A')
    const maxSubscribeId = BigInt(form['max-subscribe-id'].value)

    await moqtClient.sendSetupMessage(versions, maxSubscribeId)
  })
}

function sendSubscribeButtonClickHandler() {
  const sendSubscribeBtn = document.getElementById('sendSubscribeBtn') as HTMLButtonElement
  sendSubscribeBtn.addEventListener('click', async () => {
    const form = getFormElement()
    const trackNamespace = form['subscribe-track-namespace'].value.split('/')
    setupClientObjectCallbacks('video', 0)
    await moqtClient.subscribe(0n, 0n, trackNamespace, 'video', AUTH_INFO)

    setupClientObjectCallbacks('audio', 1)
    await moqtClient.subscribe(1n, 1n, trackNamespace, 'audio', AUTH_INFO)
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

function setupClientObjectCallbacks(type: 'video' | 'audio', trackAlias: number) {
  const alias = BigInt(trackAlias)

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

const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
connectBtn.addEventListener('click', async () => {
  const form = getFormElement()
  const url = form.url.value

  await moqtClient.connect(url, { sendSetup: false })
  const rawClient = moqtClient.getRawClient()

  if (!rawClient) {
    console.error('MOQT client not connected')
    return
  }

  rawClient.onSubgroupStreamHeader(async (_subgroupStreamHeader: any) => {
    // no-op: debugging hook
  })

  sendSetupButtonClickHandler()
  sendSubscribeButtonClickHandler()
})
