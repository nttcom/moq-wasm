import init, { MOQTClient } from '../../../pkg/moqt_client_sample'

const videoEncoderWorker = new Worker('videoEncoder.ts')
const authInfo = 'secret'
const getFormElement = (): HTMLFormElement => {
  return document.getElementById('form') as HTMLFormElement
}

let mediaStream: MediaStream | null = null

async function setUpStartGetUserMediaButton() {
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

const subgroupHeaderSent = new Set<string>()
let objectId = 0n
let groupId = 0n
let subgroupId = 0n
let isSendedSubgroupHeader = false

function sendSubgroupObjectButtonClickHandler(client: MOQTClient): void {
  const sendSubgroupObjectBtn = document.getElementById('sendSubgroupObjectBtn') as HTMLButtonElement
  sendSubgroupObjectBtn.addEventListener('click', async () => {
    if (mediaStream == null) {
      console.error('mediaStream is null')
      return
    }
    const [videoTrack] = mediaStream.getVideoTracks()
    const videoProcessor = new MediaStreamTrackProcessor({ track: videoTrack })
    const videoStream = videoProcessor.readable

    videoEncoderWorker.onmessage = async (e: MessageEvent) => {
      const form = getFormElement()
      const trackAlias = form['object-track-alias'].value
      const publisherPriority = form['publisher-priority'].value
      const key = `${groupId}:${subgroupId}`

      if (!isSendedSubgroupHeader) {
        await client.sendSubgroupStreamHeaderMessage(BigInt(trackAlias), groupId, subgroupId, publisherPriority)
        console.log('send subgroup stream header')
        isSendedSubgroupHeader = true
      }
      const { chunk, metadata } = e.data as {
        chunk: EncodedVideoChunk
        metadata: EncodedVideoChunkMetadata | undefined
      }

      const chunkData = new Uint8Array(chunk.byteLength)
      chunk.copyTo(chunkData)

      const metadataBuffer = metadata ? new TextEncoder().encode(JSON.stringify(metadata)) : new Uint8Array()

      const combinedBuffer = new Uint8Array(chunkData.length + metadataBuffer.length)
      combinedBuffer.set(chunkData, 0)
      combinedBuffer.set(metadataBuffer, chunkData.length)
      await client.sendSubgroupStreamObject(BigInt(trackAlias), groupId, subgroupId, objectId++, combinedBuffer)
    }

    videoEncoderWorker.postMessage({ videoStream: videoStream }, [videoStream])
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
