import init, { MOQTClient } from '../../../pkg/moqt_client_sample'
import { getUserMedia } from './media'

const authInfo = 'secret'
const getFormElement = (): HTMLFormElement => {
  return document.getElementById('form') as HTMLFormElement
}

async function setUpStartGetUserMediaButton() {
  const startGetUserMediaBtn = document.getElementById('startGetUserMediaBtn') as HTMLButtonElement
  startGetUserMediaBtn.addEventListener('click', async () => {
    const constraints = {
      audio: true,
      video: true
    }
    const stream = await getUserMedia(constraints)
    const video = document.getElementById('video') as HTMLVideoElement
    video.srcObject = stream
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

function sendSubgroupObjectButtonClickHandler(client: MOQTClient): void {
  const subgroupHeaderSent = new Set<string>()
  let objectId = 0n
  let groupId = 0n
  let subgroupId = 0n

  const sendSubgroupObjectBtn = document.getElementById('sendSubgroupObjectBtn') as HTMLButtonElement
  sendSubgroupObjectBtn.addEventListener('click', async () => {
    console.log('send subgroup stream object btn clicked')
    const form = getFormElement()
    const trackAlias = form['object-track-alias'].value
    const publisherPriority = form['publisher-priority'].value
    const objectPayloadString = form['object-payload'].value

    const objectPayloadArray = new TextEncoder().encode(objectPayloadString)
    const key = `${groupId}:${subgroupId}`

    console.log(trackAlias, publisherPriority, objectPayloadString, objectPayloadArray, key)
    if (!subgroupHeaderSent.has(key)) {
      console.log('send subgroup stream header')
      await client.sendSubgroupStreamHeaderMessage(BigInt(trackAlias), groupId, subgroupId, publisherPriority)
      subgroupHeaderSent.add(key)
    }
    console.log('send subgroup stream object')

    await client.sendSubgroupStreamObject(BigInt(trackAlias), groupId, subgroupId, objectId++, objectPayloadArray)
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
