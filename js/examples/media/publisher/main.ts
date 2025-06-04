import init, { MOQTClient } from '../../../pkg/moqt_client_sample'
import { AUTH_INFO, KEYFRAME_INTERVAL } from './const'
import { sendVideoObjectMessage, sendAudioObjectMessage } from './sender'
import { getFormElement } from './utils'

let mediaStream: MediaStream | null = null
function setUpStartGetUserMediaButton() {
  const startGetUserMediaBtn = document.getElementById('startGetUserMediaBtn') as HTMLButtonElement
  startGetUserMediaBtn.addEventListener('click', async () => {
    const constraints = {
      audio: false,
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

const LatestMediaTrackInfo: {
  video: {
    objectId: bigint
    groupId: bigint
    subgroups: {}
  }
  audio: {
    objectId: bigint
    groupId: bigint
    subgroups: {
      [key: number]: {
        isSendedSubgroupHeader: boolean
      }
    }
  }
} = {
  video: {
    objectId: 0n,
    groupId: -1n,
    subgroups: {
      0: {}
      // 1: {},
      // 2: {}
    }
  },
  audio: {
    objectId: 0n,
    groupId: 0n,
    subgroups: {
      0: { isSendedSubgroupHeader: false }
    }
  }
}

const videoEncoderWorker = new Worker(new URL('./videoEncoder.ts', import.meta.url), { type: 'module' })
async function handleVideoChunkMessage(
  chunk: EncodedVideoChunk,
  metadata: EncodedVideoChunkMetadata | undefined,
  client: MOQTClient
) {
  const form = getFormElement()
  const trackAlias = form['video-object-track-alias'].value
  const publisherPriority = form['video-publisher-priority'].value
  // if (LatestMediaTrackInfo['video'].objectId >= 5n) {
  //   return
  // }

  // Increment the groupId and reset the objectId at the timing of the keyframe
  // Then, resend the SubgroupStreamHeader
  if (chunk.type === 'key') {
    LatestMediaTrackInfo['video'].groupId++
    LatestMediaTrackInfo['video'].objectId = BigInt(0)

    const subgroupKeys = Object.keys(LatestMediaTrackInfo['video'].subgroups).map(BigInt)
    for (const subgroup of subgroupKeys) {
      await client.sendSubgroupStreamHeaderMessage(
        BigInt(trackAlias),
        LatestMediaTrackInfo['video'].groupId,
        // @ts-ignore - The SVC property is not defined in the standard but actually exists
        subgroup,
        publisherPriority
      )
      console.log('send subgroup stream header')
    }
  }

  sendVideoObjectMessage(
    trackAlias,
    LatestMediaTrackInfo['video'].groupId,
    // @ts-ignore - The SVC property is not defined in the standard but actually exists
    BigInt(metadata?.svc.temporalLayerId), // = subgroupId
    LatestMediaTrackInfo['video'].objectId,
    chunk,
    client
  )
  LatestMediaTrackInfo['video'].objectId++
}

const audioEncoderWorker = new Worker(new URL('./audioEncoder.ts', import.meta.url), { type: 'module' })
async function handleAudioChunkMessage(
  chunk: EncodedAudioChunk,
  metadata: EncodedAudioChunkMetadata | undefined,
  client: MOQTClient
) {
  const form = getFormElement()
  const trackAlias = form['audio-object-track-alias'].value
  const publisherPriority = form['audio-publisher-priority'].value
  const subgroupId = 0

  if (!LatestMediaTrackInfo['audio']['subgroups'][subgroupId].isSendedSubgroupHeader) {
    await client.sendSubgroupStreamHeaderMessage(
      BigInt(trackAlias),
      LatestMediaTrackInfo['audio'].groupId,
      BigInt(subgroupId),
      publisherPriority
    )
    console.log('send subgroup stream header')
    LatestMediaTrackInfo['audio']['subgroups'][subgroupId].isSendedSubgroupHeader = true
  }

  sendAudioObjectMessage(
    trackAlias,
    LatestMediaTrackInfo['audio'].groupId,
    BigInt(subgroupId),
    LatestMediaTrackInfo['audio'].objectId,
    chunk,
    client
  )
  LatestMediaTrackInfo['audio'].objectId++
}

function setupClientCallbacks(client: MOQTClient): void {
  client.onSetup(async (serverSetup: any) => {
    console.log({ serverSetup })
  })

  client.onAnnounce(async (announceMessage: any) => {
    console.log({ announceMessage })
    const announcedNamespace = announceMessage.track_namespace

    await client.sendAnnounceOkMessage(announcedNamespace)
  })

  client.onAnnounceResponce(async (announceResponceMessage: any) => {
    console.log({ announceResponceMessage })
  })

  client.onSubscribe(async (subscribeMessage: any, isSuccess: any, code: any) => {
    console.log({ subscribeMessage })
    const form = getFormElement()
    const receivedSubscribeId = BigInt(subscribeMessage.subscribe_id)
    const receivedTrackAlias = BigInt(subscribeMessage.track_alias)
    console.log('subscribeId', receivedSubscribeId, 'trackAlias', receivedTrackAlias)

    if (isSuccess) {
      const expire = 0n
      const forwardingPreference = (Array.from(form['forwarding-preference']) as HTMLInputElement[]).filter(
        (elem) => elem.checked
      )[0].value
      await client.sendSubscribeOkMessage(receivedSubscribeId, expire, AUTH_INFO, forwardingPreference)
    } else {
      const reasonPhrase = 'subscribe error'
      await client.sendSubscribeErrorMessage(subscribeMessage.subscribe_id, code, reasonPhrase)
    }
  })
}

function sendSetupButtonClickHandler(client: MOQTClient): void {
  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement
  sendSetupBtn.addEventListener('click', async () => {
    const form = getFormElement()

    const versions = new BigUint64Array('0xff00000A'.split(',').map(BigInt))
    const maxSubscribeId = BigInt(form['max-subscribe-id'].value)

    await client.sendSetupMessage(versions, maxSubscribeId)
  })
}

function sendAnnounceButtonClickHandler(client: MOQTClient): void {
  const sendAnnounceBtn = document.getElementById('sendAnnounceBtn') as HTMLButtonElement
  sendAnnounceBtn.addEventListener('click', async () => {
    const form = getFormElement()
    const trackNamespace = form['announce-track-namespace'].value.split('/')

    await client.sendAnnounceMessage(trackNamespace, AUTH_INFO)
  })
}

function sendSubgroupObjectButtonClickHandler(client: MOQTClient): void {
  const sendSubgroupObjectBtn = document.getElementById('sendSubgroupObjectBtn') as HTMLButtonElement
  sendSubgroupObjectBtn.addEventListener('click', async () => {
    console.log('clicked sendSubgroupObjectBtn')
    if (mediaStream == null) {
      console.error('mediaStream is null')
      return
    }
    videoEncoderWorker.onmessage = async (e: MessageEvent) => {
      const { chunk, metadata } = e.data as {
        chunk: EncodedVideoChunk
        metadata: EncodedVideoChunkMetadata | undefined
      }
      // console.log(chunk, metadata)
      handleVideoChunkMessage(chunk, metadata, client)
    }
    audioEncoderWorker.onmessage = async (e: MessageEvent) => {
      const { chunk, metadata } = e.data as {
        chunk: EncodedAudioChunk
        metadata: EncodedAudioChunkMetadata | undefined
      }
      // handleAudioChunkMessage(chunk, metadata, client)
    }

    const [videoTrack] = mediaStream.getVideoTracks()
    const videoProcessor = new MediaStreamTrackProcessor({ track: videoTrack })
    const videoStream = videoProcessor.readable
    videoEncoderWorker.postMessage({
      type: 'keyframeInterval',
      keyframeInterval: KEYFRAME_INTERVAL
    })
    videoEncoderWorker.postMessage({ type: 'videoStream', videoStream: videoStream }, [videoStream])
    // const [audioTrack] = mediaStream.getAudioTracks()
    // const audioProcessor = new MediaStreamTrackProcessor({ track: audioTrack })
    // const audioStream = audioProcessor.readable
    // audioEncoderWorker.postMessage({ type: 'audioStream', audioStream: audioStream }, [audioStream])
  })
}

function setupButtonClickHandler(client: MOQTClient): void {
  sendSetupButtonClickHandler(client)
  sendAnnounceButtonClickHandler(client)
  sendSubgroupObjectButtonClickHandler(client)
}

init().then(async () => {
  setUpStartGetUserMediaButton()

  const connectBtn = document.getElementById('connectBtn') as HTMLButtonElement
  connectBtn.addEventListener('click', async () => {
    const form = getFormElement()
    const url = form.url.value
    const client = new MOQTClient(url)
    setupClientCallbacks(client)
    setupButtonClickHandler(client)

    await client.start()
  })
})
