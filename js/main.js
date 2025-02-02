import init, { MOQTClient } from './pkg/moqt_client_sample'

// TODO: impl close
init().then(async () => {
  console.log('init wasm-pack')

  let headerSend = false
  let objectId = 0n
  let mutableGroupId = 0n

  const connectBtn = document.getElementById('connectBtn')
  connectBtn.addEventListener('click', async () => {
    const url = document.form.url.value
    const authInfo = form['auth-info'].value
    const receivedTextElement = document.getElementById('received-text')

    const client = new MOQTClient(url)
    console.log(client.id, client)
    console.log('URL:', client.url())

    const describeReceivedObject = (payload) => {
      // change line
      let brElement = document.createElement('br')
      receivedTextElement.prepend(brElement)

      // decode the object array to its text
      const receivedArray = new Uint8Array(payload)
      const receivedText = new TextDecoder().decode(receivedArray)

      // show received text
      let receivedElement = document.createElement('p')
      receivedElement.textContent = receivedText
      receivedTextElement.prepend(receivedElement)
    }

    client.onSetup(async (serverSetup) => {
      console.log({ serverSetup })
    })

    client.onAnnounce(async (announceMessage) => {
      console.log({ announceMessage })
      let announcedNamespace = announceMessage.track_namespace

      await client.sendAnnounceOkMessage(announcedNamespace)
    })

    client.onAnnounceResponce(async (announceResponceMessage) => {
      console.log({ announceResponceMessage })
    })

    client.onSubscribe(async (subscribeMessage, isSuccess, code) => {
      console.log({ subscribeMessage })

      let receivedSubscribeId = BigInt(subscribeMessage.subscribe_id)
      let receivedTrackAlias = BigInt(subscribeMessage.track_alias)
      console.log('subscribeId', receivedSubscribeId, 'trackAlias', receivedTrackAlias)

      if (isSuccess) {
        let expire = 0n
        const forwardingPreference = Array.from(form['forwarding-preference']).filter((elem) => elem.checked)[0].value
        await client.sendSubscribeOkMessage(receivedSubscribeId, expire, authInfo, forwardingPreference)
      } else {
        // TODO: set accurate reasonPhrase
        let reasonPhrase = 'subscribe error'
        await client.sendSubscribeError(subscribeMessage.subscribe_id, code, reasonPhrase)
      }
    })

    client.onSubscribeResponse(async (subscribeResponse) => {
      console.log({ subscribeResponse })
    })

    client.onSubscribeNamespaceResponse(async (subscribeNamespaceResponse) => {
      console.log({ subscribeNamespaceResponse })
    })

    client.onDatagramObject(async (datagramObject) => {
      console.log({ datagramObject })
      describeReceivedObject(datagramObject.object_payload)
    })

    client.onStreamHeaderTrack(async (streamHeaderTrack) => {
      console.log({ streamHeaderTrack })
    })

    client.onObjectStreamTrack(async (objectStreamTrack) => {
      console.log({ objectStreamTrack })
      describeReceivedObject(objectStreamTrack.object_payload)
    })

    client.onStreamHeaderSubgroup(async (streamHeaderSubgroup) => {
      console.log({ streamHeaderSubgroup })
    })

    client.onObjectStreamSubgroup(async (objectStreamSubgroup) => {
      console.log({ objectStreamSubgroup })
      describeReceivedObject(objectStreamSubgroup.object_payload)
    })

    const objectIdElement = document.getElementById('objectId')
    const mutableGroupIdElement = document.getElementById('mutableGroupId')

    const sendSetupBtn = document.getElementById('sendSetupBtn')
    sendSetupBtn.addEventListener('click', async () => {
      console.log('send setup btn clicked')
      const role = Array.from(form['role']).filter((elem) => elem.checked)[0].value
      const versions = form['versions'].value.split(',').map(BigInt)
      const maxSubscribeId = form['max-subscribe-id'].value

      await client.sendSetupMessage(role, versions, BigInt(maxSubscribeId))
    })

    const sendAnnounceBtn = document.getElementById('sendAnnounceBtn')
    sendAnnounceBtn.addEventListener('click', async () => {
      console.log('send announce btn clicked')
      const trackNamespace = form['announce-track-namespace'].value.split('/')
      const authInfo = form['auth-info'].value

      await client.sendAnnounceMessage(trackNamespace, authInfo)
    })

    const sendSubscribeNamespaceBtn = document.getElementById('sendSubscribeNamespaceBtn')
    sendSubscribeNamespaceBtn.addEventListener('click', async () => {
      console.log('send subscribe namespace btn clicked')
      const trackNamespacePrefix = form['track-namespace-prefix'].value.split('/')
      const authInfo = form['auth-info'].value

      await client.sendSubscribeNamespaceMessage(trackNamespacePrefix, authInfo)
    })

    const sendSubscribeBtn = document.getElementById('sendSubscribeBtn')
    sendSubscribeBtn.addEventListener('click', async () => {
      console.log('send subscribe btn clicked')
      const subscribeId = form['subscribe-subscribe-id'].value
      const trackAlias = form['subscribe-track-alias'].value
      const trackNamespace = form['subscribe-track-namespace'].value.split('/')
      const trackName = form['track-name'].value
      const subscriberPriority = form['subscriber-priority'].value
      const groupOrder = Array.from(form['group-order']).filter((elem) => elem.checked)[0].value
      const filterType = Array.from(form['filter-type']).filter((elem) => elem.checked)[0].value
      const startGroup = form['start-group'].value
      const startObject = form['start-object'].value
      const endGroup = form['end-group'].value
      const endObject = form['end-object'].value

      const authInfo = form['auth-info'].value

      await client.sendSubscribeMessage(
        BigInt(subscribeId),
        BigInt(trackAlias),
        trackNamespace,
        trackName,
        subscriberPriority,
        groupOrder,
        filterType,
        BigInt(startGroup),
        BigInt(startObject),
        BigInt(endGroup),
        BigInt(endObject),
        authInfo
      )
    })

    const sendDatagramObjectBtn = document.getElementById('sendDatagramObjectBtn')
    sendDatagramObjectBtn.addEventListener('click', async () => {
      console.log('send datagram object btn clicked')
      const subscribeId = form['object-subscribe-id'].value
      const trackAlias = form['object-track-alias'].value
      const publisherPriority = form['publisher-priority'].value
      const objectPayloadString = form['object-payload'].value

      // encode the text to the object array
      const objectPayloadArray = new TextEncoder().encode(objectPayloadString)

      await client.sendDatagramObject(
        BigInt(subscribeId),
        BigInt(trackAlias),
        mutableGroupId,
        objectId++,
        publisherPriority,
        objectPayloadArray
      )
      objectIdElement.textContent = objectId
    })

    const sendTrackObjectBtn = document.getElementById('sendTrackObjectBtn')
    sendTrackObjectBtn.addEventListener('click', async () => {
      console.log('send track stream object btn clicked')
      const subscribeId = form['object-subscribe-id'].value
      const trackAlias = form['object-track-alias'].value
      const publisherPriority = form['publisher-priority'].value
      const objectPayloadString = form['object-payload'].value

      // encode the text to the object array
      const objectPayloadArray = new TextEncoder().encode(objectPayloadString)

      // send header if it is the first time
      if (!headerSend) {
        await client.sendStreamHeaderTrackMessage(BigInt(subscribeId), BigInt(trackAlias), publisherPriority)
        headerSend = true
      }

      await client.sendObjectStreamTrack(BigInt(subscribeId), mutableGroupId, objectId++, objectPayloadArray)
      objectIdElement.textContent = objectId
    })

    const sendSubgroupObjectBtn = document.getElementById('sendSubgroupObjectBtn')
    sendSubgroupObjectBtn.addEventListener('click', async () => {
      console.log('send subgroup stream object btn clicked')
      const subscribeId = form['object-subscribe-id'].value
      const groupId = form['subgroup-group-id'].value
      const subgroupId = form['subgroup-id'].value
      const trackAlias = form['object-track-alias'].value
      const publisherPriority = form['publisher-priority'].value
      const objectPayloadString = form['object-payload'].value

      // encode the text to the object array
      const objectPayloadArray = new TextEncoder().encode(objectPayloadString)

      // send header if it is the first time
      if (!headerSend) {
        await client.sendStreamHeaderSubgroupMessage(
          BigInt(subscribeId),
          BigInt(trackAlias),
          BigInt(groupId),
          BigInt(subgroupId),
          publisherPriority
        )
        headerSend = true
      }

      await client.sendObjectStreamSubgroup(subscribeId, objectId++, objectPayloadArray)
      objectIdElement.textContent = objectId
    })

    const ascendMutableGroupId = document.getElementById('ascendMutableGroupIdBtn')
    ascendMutableGroupId.addEventListener('click', async () => {
      mutableGroupId++
      objectId = 0n
      console.log('ascend mutableGroupId', mutableGroupId)

      mutableGroupIdElement.textContent = mutableGroupId
      objectIdElement.textContent = objectId
    })

    const descendMutableGroupId = document.getElementById('descendMutableGroupIdBtn')
    descendMutableGroupId.addEventListener('click', async () => {
      if (mutableGroupId === 0n) {
        return
      }
      mutableGroupId--
      objectId = 0n
      console.log('descend mutableGroupId', mutableGroupId)
      mutableGroupIdElement.textContent = mutableGroupId
      objectIdElement.textContent = objectId
    })

    await client.start()
  })

  const forwardingPreference = document.querySelectorAll('input[name="forwarding-preference"]')
  const subgroupHeaderContents = document.getElementById('subgroupHeaderContents')
  const notSubgroupObjectContents = document.getElementById('notSubgroupObjectContents')
  const sendDatagramObject = document.getElementById('sendDatagramObject')
  const sendTrackObject = document.getElementById('sendTrackObject')
  const sendSubgroupObject = document.getElementById('sendSubgroupObject')
  const headerField = document.getElementById('headerField')
  const objectField = document.getElementById('objectField')

  // change ui within track/subgroup
  forwardingPreference.forEach((elem) => {
    elem.addEventListener('change', async () => {
      if (elem.value === 'datagram') {
        notSubgroupObjectContents.style.display = 'block'
        subgroupHeaderContents.style.display = 'none'
        sendDatagramObject.style.display = 'block'
        sendTrackObject.style.display = 'none'
        sendSubgroupObject.style.display = 'none'
        headerField.style.display = 'none'
        objectField.style.display = 'none'
      } else if (elem.value === 'track') {
        notSubgroupObjectContents.style.display = 'block'
        subgroupHeaderContents.style.display = 'none'
        sendDatagramObject.style.display = 'none'
        sendTrackObject.style.display = 'block'
        sendSubgroupObject.style.display = 'none'
        headerField.style.display = 'block'
        objectField.style.display = 'block'
      } else if (elem.value === 'subgroup') {
        notSubgroupObjectContents.style.display = 'none'
        subgroupHeaderContents.style.display = 'block'
        sendDatagramObject.style.display = 'none'
        sendTrackObject.style.display = 'none'
        sendSubgroupObject.style.display = 'block'
        headerField.style.display = 'block'
        objectField.style.display = 'block'
      }
    })
  })
})
