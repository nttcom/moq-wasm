import init, { MOQTClient } from '../../../pkg/moqt_client_sample'

const videoDecoderWorker = new Worker('videoDecoder.ts')
const audioDecoderWorker = new Worker('audioDecoder.ts')
// videoFrameをvideoタグに表示する処理を追加
const videoElement = document.getElementById('video') as HTMLFormElement
const videoGenerator = new MediaStreamTrackGenerator({ kind: 'video' })
const videoWriter = videoGenerator.writable.getWriter()
const videoStream = new MediaStream([videoGenerator])
videoElement.srcObject = videoStream

const audioElement = document.getElementById('audio') as HTMLFormElement
const audioGenerator = new MediaStreamTrackGenerator({ kind: 'audio' })
const audioWriter = audioGenerator.writable.getWriter()
const audioStream = new MediaStream([audioGenerator])
audioElement.srcObject = audioStream

videoDecoderWorker.onmessage = async (e: MessageEvent) => {
  const videoFrame = e.data.frame
  console.log(e.data)
  await videoWriter.write(videoFrame)
  videoFrame.close()
}

audioDecoderWorker.onmessage = async (e: MessageEvent) => {
  const audioData = e.data.audioData
  console.log(audioData)
  await audioWriter.write(audioData)
  audioElement.play()
}

const authInfo = 'secret'
const getFormElement = (): HTMLFormElement => {
  return document.getElementById('form') as HTMLFormElement
}

function setupClientObjectCallbacks(client: MOQTClient, type: 'video' | 'audio', trackAlias: number) {
  client.onSubgroupStreamHeader(async (subgroupStreamHeader: any) => {
    console.log({ subgroupStreamHeader })
  })

  client.onSubgroupStreamObject(BigInt(trackAlias), async (subgroupStreamObject: any) => {
    if (type === 'video') {
      videoDecoderWorker.postMessage({ subgroupStreamObject })
    } else {
      audioDecoderWorker.postMessage({ subgroupStreamObject })
    }
  })
}

function setupClientCallbacks(client: MOQTClient) {
  client.onSetup(async (serverSetup: any) => {
    console.log({ serverSetup })
  })
}

function sendSetupButtonClickHandler(client: MOQTClient) {
  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement
  sendSetupBtn.addEventListener('click', async () => {
    const form = getFormElement()

    const role = 2
    const versions = new BigUint64Array('0xff000008'.split(',').map(BigInt))
    const maxSubscribeId = BigInt(form['max-subscribe-id'].value)

    await client.sendSetupMessage(role, versions, maxSubscribeId)
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
      BigInt(10000), // endObject
      authInfo
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
      BigInt(10000), // endObject
      authInfo
    )
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
