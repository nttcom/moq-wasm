import init, { MOQTClient } from '../../../pkg/moqt_client_sample'
import { sendVideoObjectMessage, sendAudioObjectMessage } from './sender'

let mediaStream: MediaStream | null = null
function setUpStartGetUserMediaButton() {
  const startGetUserMediaBtn = document.getElementById('startGetUserMediaBtn') as HTMLButtonElement
  startGetUserMediaBtn.addEventListener('click', async () => {
    const constraints = {
      audio: true,
      video: true
    }
    mediaStream = await navigator.mediaDevices.getUserMedia(constraints)
    const video = document.getElementById('video') as HTMLVideoElement
    video.srcObject = mediaStream
  })
}

const LatestMediaTrackInfo = {
  video: {
    objectId: 0n,
    groupId: 0n,
    subgroupId: 0n,
    isSendedSubgroupHeader: false
  },
  audio: {
    objectId: 0n,
    groupId: 0n,
    subgroupId: 0n,
    isSendedSubgroupHeader: false
  }
}

const videoEncoderWorker = new Worker('videoEncoder.ts')
async function handleVideoChunkMessage(
  chunk: EncodedVideoChunk,
  metadata: EncodedVideoChunkMetadata | undefined,
  client: MOQTClient
) {
  const form = getFormElement()
  const trackAlias = form['video-object-track-alias'].value
  const publisherPriority = form['video-publisher-priority'].value
  // const key = `${groupId}:${subgroupId}`

  if (chunk.type === 'key') {
    LatestMediaTrackInfo['video'].groupId++
    LatestMediaTrackInfo['video'].objectId = BigInt(0)
    LatestMediaTrackInfo['video'].isSendedSubgroupHeader = false
  } else {
    LatestMediaTrackInfo['video'].objectId++
  }

  if (!LatestMediaTrackInfo['video'].isSendedSubgroupHeader) {
    await client.sendSubgroupStreamHeaderMessage(
      BigInt(trackAlias),
      LatestMediaTrackInfo['video'].groupId,
      LatestMediaTrackInfo['video'].subgroupId,
      publisherPriority
    )
    console.log('send subgroup stream header')
    LatestMediaTrackInfo['video'].isSendedSubgroupHeader = true
  }

  LatestMediaTrackInfo['video'].objectId++
  sendVideoObjectMessage(
    trackAlias,
    LatestMediaTrackInfo['video'].groupId,
    LatestMediaTrackInfo['video'].subgroupId,
    LatestMediaTrackInfo['video'].objectId,
    chunk,
    metadata,
    client
  )
}

const audioEncoderWorker = new Worker('audioEncoder.ts')
async function handleAudioChunkMessage(
  chunk: EncodedAudioChunk,
  metadata: EncodedAudioChunkMetadata | undefined,
  client: MOQTClient
) {
  const form = getFormElement()
  const trackAlias = form['audio-object-track-alias'].value
  const publisherPriority = form['audio-publisher-priority'].value
  // const key = `${groupId}:${subgroupId}`

  if (!LatestMediaTrackInfo['audio'].isSendedSubgroupHeader) {
    await client.sendSubgroupStreamHeaderMessage(
      BigInt(trackAlias),
      LatestMediaTrackInfo['audio'].groupId,
      LatestMediaTrackInfo['audio'].subgroupId,
      publisherPriority
    )
    console.log('send subgroup stream header')
    LatestMediaTrackInfo['audio'].isSendedSubgroupHeader = true
  }

  LatestMediaTrackInfo['audio'].objectId++
  sendAudioObjectMessage(
    trackAlias,
    LatestMediaTrackInfo['audio'].groupId,
    LatestMediaTrackInfo['audio'].subgroupId,
    LatestMediaTrackInfo['audio'].objectId,
    chunk,
    metadata,
    client
  )
}

const authInfo = 'secret'
const getFormElement = (): HTMLFormElement => {
  return document.getElementById('form') as HTMLFormElement
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
      await client.sendSubscribeOkMessage(receivedSubscribeId, expire, authInfo, forwardingPreference)
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

    const role = 1
    const versions = new BigUint64Array('0xff000008'.split(',').map(BigInt))
    const maxSubscribeId = BigInt(form['max-subscribe-id'].value)

    await client.sendSetupMessage(role, versions, maxSubscribeId)
  })
}

function sendAnnounceButtonClickHandler(client: MOQTClient): void {
  const sendAnnounceBtn = document.getElementById('sendAnnounceBtn') as HTMLButtonElement
  sendAnnounceBtn.addEventListener('click', async () => {
    const form = getFormElement()
    const trackNamespace = form['announce-track-namespace'].value.split('/')

    await client.sendAnnounceMessage(trackNamespace, authInfo)
  })
}

function sendSubgroupObjectButtonClickHandler(client: MOQTClient): void {
  const sendSubgroupObjectBtn = document.getElementById('sendSubgroupObjectBtn') as HTMLButtonElement
  sendSubgroupObjectBtn.addEventListener('click', async () => {
    if (mediaStream == null) {
      console.error('mediaStream is null')
      return
    }
    videoEncoderWorker.onmessage = async (e: MessageEvent) => {
      const { chunk, metadata } = e.data as {
        chunk: EncodedVideoChunk
        metadata: EncodedVideoChunkMetadata | undefined
      }
      handleVideoChunkMessage(chunk, metadata, client)
    }
    audioEncoderWorker.onmessage = async (e: MessageEvent) => {
      const { chunk, metadata } = e.data as {
        chunk: EncodedAudioChunk
        metadata: EncodedAudioChunkMetadata | undefined
      }
      handleAudioChunkMessage(chunk, metadata, client)
    }

    const [videoTrack] = mediaStream.getVideoTracks()
    const videoProcessor = new MediaStreamTrackProcessor({ track: videoTrack })
    const videoStream = videoProcessor.readable
    videoEncoderWorker.postMessage({ videoStream: videoStream }, [videoStream])
    const [audioTrack] = mediaStream.getAudioTracks()
    const audioProcessor = new MediaStreamTrackProcessor({ track: audioTrack })
    const audioStream = audioProcessor.readable
    audioEncoderWorker.postMessage({ audioStream: audioStream }, [audioStream])
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
