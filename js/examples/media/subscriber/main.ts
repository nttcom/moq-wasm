import init, { MOQTClient } from '../../../pkg/moqt_client_sample'

const videoDecoderWorker = new Worker('videoDecoder.ts')
// videoFrameをvideoタグに表示する処理を追加
const videoElement = document.getElementById('video') as HTMLFormElement
const generator = new MediaStreamTrackGenerator({ kind: 'video' })
const writer = generator.writable.getWriter()
const stream = new MediaStream([generator])
videoElement.srcObject = stream

videoDecoderWorker.onmessage = async (e: MessageEvent) => {
  const videoFrame = e.data.frame
  console.log(e.data)
  await writer.write(videoFrame)
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
    console.log(subgroupStreamObject)
    if (type === 'video') {
      videoDecoderWorker.postMessage({ subgroupStreamObject })
    } else {
      console.log('audio')
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
    const trackNamespace = 'video_audio'.split('/')
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
