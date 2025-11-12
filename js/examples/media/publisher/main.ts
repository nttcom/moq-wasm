import { MoqtClientWrapper } from '@moqt/moqtClient'
import type { MOQTClient } from '../../../pkg/moqt_client_sample'
import { AUTH_INFO } from './const'
import { sendVideoObjectMessage, sendAudioObjectMessage } from './sender'
import { getFormElement } from './utils'
import { MediaTransportState } from '../../../utils/media/transportState'
import { sendVideoChunkViaMoqt } from '../../../utils/media/videoTransport'
import { KEYFRAME_INTERVAL } from '../../../utils/media/constants'

let mediaStream: MediaStream | null = null
const moqtClient = new MoqtClientWrapper()
const transportState = new MediaTransportState()

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
const audioEncoderWorker = new Worker(new URL('../../../utils/media/encoders/audioEncoder.ts', import.meta.url), {
  type: 'module'
})

type VideoEncoderWorkerMessage =
  | { type: 'chunk'; chunk: EncodedVideoChunk; metadata: EncodedVideoChunkMetadata | undefined }
  | { type: 'bitrate'; kbps: number }

type AudioEncoderWorkerMessage =
  | { type: 'chunk'; chunk: EncodedAudioChunk; metadata: EncodedAudioChunkMetadata | undefined }
  | { type: 'bitrate'; media: 'audio'; kbps: number }
async function handleAudioChunkMessage(
  chunk: EncodedAudioChunk,
  metadata: EncodedAudioChunkMetadata | undefined,
  client: MOQTClient
) {
  const form = getFormElement()
  const trackAlias = BigInt(form['audio-object-track-alias'].value)
  const publisherPriority = Number(form['audio-publisher-priority'].value)
  const subgroupId = 0
  transportState.ensureAudioSubgroup(subgroupId)

  if (transportState.shouldSendAudioHeader(trackAlias, subgroupId)) {
    await client.sendSubgroupStreamHeaderMessage(
      trackAlias,
      transportState.getAudioGroupId(),
      BigInt(subgroupId),
      publisherPriority
    )
    console.log('send subgroup stream header')
    transportState.markAudioHeaderSent(trackAlias, subgroupId)
  }

  sendAudioObjectMessage(
    trackAlias,
    transportState.getAudioGroupId(),
    BigInt(subgroupId),
    transportState.getAudioObjectId(),
    chunk,
    client
  )
  transportState.incrementAudioObject()
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
    const trackNamespace = form['announce-track-namespace'].value.split('/')

    await moqtClient.announce(trackNamespace, AUTH_INFO)
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
      const { chunk, metadata } = data
      const form = getFormElement()
      const trackAlias = BigInt(form['video-object-track-alias'].value)
      const publisherPriority = Number(form['video-publisher-priority'].value)

      await sendVideoChunkViaMoqt({
        chunk,
        metadata,
        trackAliases: [trackAlias],
        publisherPriority,
        client,
        transportState,
        sender: sendVideoObjectMessage
      })
    }
    audioEncoderWorker.onmessage = async (event: MessageEvent<AudioEncoderWorkerMessage>) => {
      const data = event.data
      if (data.type !== 'chunk') {
        return
      }
      const { chunk, metadata } = data
      handleAudioChunkMessage(chunk, metadata, client)
    }

    const [videoTrack] = mediaStream.getVideoTracks()
    const videoProcessor = new MediaStreamTrackProcessor({ track: videoTrack })
    const videoStream = videoProcessor.readable
    videoEncoderWorker.postMessage({
      type: 'keyframeInterval',
      keyframeInterval: KEYFRAME_INTERVAL
    })
    videoEncoderWorker.postMessage({ type: 'videoStream', videoStream: videoStream }, [videoStream])
    const [audioTrack] = mediaStream.getAudioTracks()
    const audioProcessor = new MediaStreamTrackProcessor({ track: audioTrack })
    const audioStream = audioProcessor.readable
    audioEncoderWorker.postMessage({ type: 'audioStream', audioStream: audioStream }, [audioStream])
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
    const forwardingPreference = (Array.from(form['forwarding-preference']) as HTMLInputElement[]).filter(
      (elem) => elem.checked
    )[0].value

    if (isSuccess) {
      const expire = 0n
      await respondOk(expire, AUTH_INFO, forwardingPreference)
      return
    }

    const reasonPhrase = 'subscribe error'
    await respondError(BigInt(code), reasonPhrase)
  })
}

setUpStartGetUserMediaButton()
setupClientCallbacks()

const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
connectBtn.addEventListener('click', async () => {
  const form = getFormElement()
  const url = form.url.value

  await moqtClient.connect(url, { sendSetup: false })
  setupButtonClickHandler()
})
