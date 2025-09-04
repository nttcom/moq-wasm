import init, { MOQTClient } from '../../../pkg/moqt_client_sample'
import { AUTH_INFO } from './const'
import { getFormElement } from './utils'

function setupClientCallbacks(client: MOQTClient) {
  client.onSetup(async (serverSetup: any) => {
    console.log({ serverSetup })
  })
}

function sendSetupButtonClickHandler(client: MOQTClient) {
  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement
  sendSetupBtn.addEventListener('click', async () => {
    const form = getFormElement()

    const versions = new BigUint64Array('0xff000007'.split(',').map(BigInt))
    const role = 0x02 // subscriber
    const maxSubscribeId = BigInt(form['max-subscribe-id'].value)

    await client.sendSetupMessage(versions, role, maxSubscribeId)
  })
}

function sendSubscribeButtonClickHandler(client: MOQTClient) {
  const sendSubscribeBtn = document.getElementById('sendSubscribeBtn') as HTMLButtonElement
  sendSubscribeBtn.addEventListener('click', async () => {
    const form = getFormElement()
    const trackNamespace = form['subscribe-track-namespace'].value.split('/')
    setupClientObjectCallbacks(client, 'video', Number(0))
    await client.sendSubscribeMessage(
      BigInt(0),
      BigInt(0),
      trackNamespace,
      'video',
      0, // subscriberPriority
      0, // groupOrder
      1, // Latest Group
      BigInt(0), // startGroup
      BigInt(0), // startObject
      BigInt(10000), // endGroup
      AUTH_INFO
    )

    setupClientObjectCallbacks(client, 'audio', Number(1))
    await client.sendSubscribeMessage(
      BigInt(1),
      BigInt(1),
      trackNamespace,
      'audio',
      0, // subscriberPriority
      0, // groupOrder
      1, // Latest Group
      BigInt(0), // startGroup
      BigInt(0), // startObject
      BigInt(10000), // endGroup
      AUTH_INFO
    )
  })
}

const audioDecoderWorker = new Worker(new URL('./audioDecoder.ts', import.meta.url), { type: 'module' })
function setupAudioDecoderWorker() {
  const audioGenerator = new MediaStreamTrackGenerator({ kind: 'audio' })
  const audioWriter = audioGenerator.writable.getWriter()
  const audioStream = new MediaStream([audioGenerator])
  const audioElement = document.getElementById('audio') as HTMLFormElement
  audioElement.srcObject = audioStream
  audioDecoderWorker.onmessage = async (e: MessageEvent) => {
    const audioData = e.data.audioData
    await audioWriter.write(audioData)
    await audioElement.play()
  }
}
const videoDecoderWorker = new Worker(new URL('./videoDecoder.ts', import.meta.url), { type: 'module' })
function setupVideoDecoderWorker() {
  const videoGenerator = new MediaStreamTrackGenerator({ kind: 'video' })
  const videoWriter = videoGenerator.writable.getWriter()
  const videoStream = new MediaStream([videoGenerator])
  const videoElement = document.getElementById('video') as HTMLFormElement
  videoElement.srcObject = videoStream
  videoDecoderWorker.onmessage = async (e: MessageEvent) => {
    const videoFrame = e.data.frame
    await videoWriter.write(videoFrame)
    videoFrame.close()
    await videoElement.play()
  }
}

function setupClientObjectCallbacks(client: MOQTClient, type: 'video' | 'audio', trackAlias: number) {
  client.onSubgroupStreamHeader(async (subgroupStreamHeader: any) => {
    // console.log({ subgroupStreamHeader })
  })

  if (type === 'audio') {
    setupAudioDecoderWorker()
  } else {
    setupVideoDecoderWorker()
  }

  client.onSubgroupStreamObject(BigInt(trackAlias), async (groupId: number, subgroupStreamObject: any) => {
    // WARNING: Use only debug for memory usage
    // console.log(subgroupStreamObject.object_id)
    console.log(subgroupStreamObject)
    // if (type === 'video') {
    //   if (
    //     subgroupStreamObject.objectPayloadLength === 0 ||
    //     subgroupStreamObject.object_status === 'EndOfGroup' ||
    //     subgroupStreamObject.object_status === 'EndOfTrackAndGroup' ||
    //     subgroupStreamObject.object_status === 'EndOfTrack'
    //   ) {
    //     // WARNING: Use only debug for memory usage
    //     // console.log(subgroupStreamObject)
    //     return
    //   }

    //   videoDecoderWorker.postMessage({
    //     groupId,
    //     subgroupStreamObject
    //   })
    // } else {
    //   audioDecoderWorker.postMessage({
    //     groupId,
    //     subgroupStreamObject
    //   })
    // }
  })
}

function setupButtonClickHandler(client: MOQTClient) {
  sendSetupButtonClickHandler(client)
  sendSubscribeButtonClickHandler(client)
}

init().then(async () => {
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
