import init, { MOQTClient } from '../../../pkg/moqt_client_sample'
import { getUserMedia } from './media'

const authInfo = 'secret'

function startGetUserMedia() {
  const constraints = {
    audio: true,
    video: true
  }
  const stream = getUserMedia(constraints)
}

function setupClientCallbacks(client) {
  client.onSetup(async (serverSetup) => {
    console.log({ serverSetup })
  })

  client.onAnnounce(async (announceMessage) => {
    console.log({ announceMessage })
    const announcedNamespace = announceMessage.track_namespace

    await client.sendAnnounceOkMessage(announcedNamespace)
  })

  client.onAnnounceResponce(async (announceResponceMessage) => {
    console.log({ announceResponceMessage })
  })

  client.onSubscribe(async (subscribeMessage, isSuccess, code) => {
    console.log({ subscribeMessage })

    const receivedSubscribeId = BigInt(subscribeMessage.subscribe_id)
    const receivedTrackAlias = BigInt(subscribeMessage.track_alias)
    console.log('subscribeId', receivedSubscribeId, 'trackAlias', receivedTrackAlias)

    if (isSuccess) {
      const expire = 0n
      const forwardingPreference = Array.from(form['forwarding-preference']).filter((elem) => elem.checked)[0].value
      await client.sendSubscribeOkMessage(receivedSubscribeId, expire, authInfo, forwardingPreference)
    } else {
      // TODO: set accurate reasonPhrase
      const reasonPhrase = 'subscribe error'
      await client.sendSubscribeError(subscribeMessage.subscribe_id, code, reasonPhrase)
    }
  })
}

function sendSetupButtonClickHandler(client) {
  const sendSetupBtn = document.getElementById('sendSetupBtn')
  sendSetupBtn.addEventListener('click', async () => {
    const role = 1
    const versions = '0xff000008'.split(',').map(BigInt)
    const maxSubscribeId = BigInt(form['max-subscribe-id'].value)

    await client.sendSetupMessage(role, versions, maxSubscribeId)
  })
}

function sendAnnounceButtonClickHandler(client) {
  const sendAnnounceBtn = document.getElementById('sendAnnounceBtn')
  sendAnnounceBtn.addEventListener('click', async () => {
    const trackNamespace = form['announce-track-namespace'].value.split('/')

    await client.sendAnnounceMessage(trackNamespace, authInfo)
  })
}

function sendSubgroupObjectButtonClickHandler(client) {
  const subgroupHeaderSent = new Set()
  let objectId = 0n
  let groupId = 0n
  let subgroupId = 0n

  const sendSubgroupObjectBtn = document.getElementById('sendSubgroupObjectBtn')
  sendSubgroupObjectBtn.addEventListener('click', async () => {
    console.log('send subgroup stream object btn clicked')
    const trackAlias = form['object-track-alias'].value
    const publisherPriority = form['publisher-priority'].value
    const objectPayloadString = form['object-payload'].value

    // encode the text to the object array
    const objectPayloadArray = new TextEncoder().encode(objectPayloadString)
    const key = `${groupId}:${subgroupId}`

    console.log(trackAlias, publisherPriority, objectPayloadString, objectPayloadArray, key)
    // send header if it is the first time
    if (!subgroupHeaderSent.has(key)) {
      console.log('send subgroup stream header')
      await client.sendSubgroupStreamHeaderMessage(BigInt(trackAlias), groupId, subgroupId, publisherPriority)
      subgroupHeaderSent.add(key)
    }
    console.log('send subgroup stream object')

    await client.sendSubgroupStreamObject(BigInt(trackAlias), groupId, subgroupId, objectId++, objectPayloadArray)
  })
}

function setupButtonClickHandler(client) {
  sendSetupButtonClickHandler(client)
  sendAnnounceButtonClickHandler(client)
  sendSubgroupObjectButtonClickHandler(client)
}

init().then(async () => {
  const connectBtn = document.getElementById('connectBtn')
  connectBtn.addEventListener('click', async () => {
    const url = document.form.url.value
    const client = new MOQTClient(url)
    setupClientCallbacks(client)
    setupButtonClickHandler(client)
    await client.start()
  })
})
