import init, { MOQTClient } from '../../pkg/moqt_client_sample'

// TODO: impl close
init().then(async () => {
  console.log('init wasm-pack')

  const subgroupHeaderSent = new Set()
  let objectId = 0n
  let groupId = 0n
  let subgroupId = 0n

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
      const brElement = document.createElement('br')
      receivedTextElement.prepend(brElement)

      // decode the object array to its text
      const receivedArray = new Uint8Array(payload)
      const receivedText = new TextDecoder().decode(receivedArray)

      // show received text
      const receivedElement = document.createElement('p')
      receivedElement.textContent = receivedText
      receivedTextElement.prepend(receivedElement)
    }

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

    client.onSubscribeResponse(async (subscribeResponse) => {
      console.log({ subscribeResponse })
    })

    client.onSubscribeAnnouncesResponse(async (subscribeAnnouncesResponse) => {
      console.log({ subscribeAnnouncesResponse })
    })

    client.onUnsubscribe(async (unsubscribeMessage) => {
      console.log({ unsubscribeMessage })
    })

    client.onDatagramObject(async (datagramObject) => {
      console.log({ datagramObject })
      describeReceivedObject(datagramObject.object_payload)
    })

    client.onDatagramObjectStatus(async (datagramObjectStatus) => {
      console.log({ datagramObjectStatus })
    })

    client.onSubgroupStreamHeader(async (subgroupStreamHeader) => {
      console.log({ subgroupStreamHeader })
    })

    const trackAlias = form['subscribe-track-alias'].value
    client.onSubgroupStreamObject(BigInt(trackAlias), async (groupId, subgroupStreamObject) => {
      console.log({ subgroupStreamObject })
      describeReceivedObject(subgroupStreamObject.object_payload)
    })

    const objectIdElement = document.getElementById('objectId')
    const datagramGroupIdElement = document.getElementById('datagramGroupId')
    const subgroupGroupIdElement = document.getElementById('subgroupGroupId')
    const subgroupIdElement = document.getElementById('subgroupId')

    const sendSetupBtn = document.getElementById('sendSetupBtn')
    sendSetupBtn.addEventListener('click', async () => {
      console.log('send setup btn clicked')
      const versions = form['versions'].value.split(',').map(BigInt)
      const role = Array.from(form['role']).filter((elem) => elem.checked)[0].value
      console.log(role)
      const maxSubscribeId = form['max-subscribe-id'].value

      await client.sendSetupMessage(versions, role, BigInt(maxSubscribeId))
    })

    const sendAnnounceBtn = document.getElementById('sendAnnounceBtn')
    sendAnnounceBtn.addEventListener('click', async () => {
      console.log('send announce btn clicked')
      const trackNamespace = form['announce-track-namespace'].value.split('/')
      const authInfo = form['auth-info'].value

      await client.sendAnnounceMessage(trackNamespace, authInfo)
    })

    const sendSubscribeAnnouncesBtn = document.getElementById('sendSubscribeAnnouncesBtn')
    sendSubscribeAnnouncesBtn.addEventListener('click', async () => {
      console.log('send subscribe announces btn clicked')
      const trackNamespacePrefix = form['track-namespace-prefix'].value.split('/')
      const authInfo = form['auth-info'].value

      await client.sendSubscribeAnnouncesMessage(trackNamespacePrefix, authInfo)
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

      const authInfo = form['auth-info'].value
      console.log(
        subscribeId,
        trackAlias,
        trackNamespace,
        trackName,
        subscriberPriority,
        groupOrder,
        filterType,
        startGroup,
        startObject,
        endGroup
      )

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
        authInfo
      )
    })

    const sendUnsubscribeBtn = document.getElementById('sendUnsubscribeBtn')
    sendUnsubscribeBtn.addEventListener('click', async () => {
      console.log('send unsubscribe btn clicked')
      const subscribeId = form['unsubscribe-subscribe-id'].value
      await client.sendUnsubscribeMessage(subscribeId)
    })

    const sendDatagramObjectBtn = document.getElementById('sendDatagramObjectBtn')
    sendDatagramObjectBtn.addEventListener('click', async () => {
      console.log('send datagram object btn clicked')
      const trackAlias = form['object-track-alias'].value
      const publisherPriority = form['publisher-priority'].value
      const objectPayloadString = form['object-payload'].value

      // encode the text to the object array
      const objectPayloadArray = new TextEncoder().encode(objectPayloadString)

      await client.sendDatagramObject(BigInt(trackAlias), groupId, objectId++, publisherPriority, objectPayloadArray)
      objectIdElement.textContent = objectId
    })

    const sendDatagramObjectWithStatusBtn = document.getElementById('sendDatagramObjectWithStatusBtn')
    sendDatagramObjectWithStatusBtn.addEventListener('click', async () => {
      console.log('send datagram object with status btn clicked')
      const trackAlias = form['object-track-alias'].value
      const publisherPriority = form['publisher-priority'].value
      const objectStatus = Array.from(form['object-status']).filter((elem) => elem.checked)[0].value

      await client.sendDatagramObjectStatus(BigInt(trackAlias), groupId, objectId++, publisherPriority, objectStatus)
      objectIdElement.textContent = objectId
    })

    const sendSubgroupObjectHeaderBtn = document.getElementById('sendSubgroupObjectHeaderBtn')
    sendSubgroupObjectHeaderBtn.addEventListener('click', async () => {
      console.log('send subgroup stream object header btn clicked')
      const trackAlias = form['object-track-alias'].value
      const publisherPriority = form['publisher-priority'].value

      await client.sendSubgroupStreamHeaderMessage(BigInt(trackAlias), groupId, subgroupId, publisherPriority)
    })

    const sendSubgroupObjectBtn = document.getElementById('sendSubgroupObjectBtn')
    sendSubgroupObjectBtn.addEventListener('click', async () => {
      console.log('send subgroup stream object btn clicked')
      const trackAlias = form['object-track-alias'].value
      const objectPayloadString = form['object-payload'].value

      // encode the text to the object array
      const objectPayloadArray = new TextEncoder().encode(objectPayloadString)
      const key = `${groupId}:${subgroupId}`

      await client.sendSubgroupStreamObject(
        BigInt(trackAlias),
        groupId,
        subgroupId,
        objectId,
        undefined,
        // Uint8Array.from([])
        objectPayloadArray
      )
      objectId++
      objectIdElement.textContent = objectId
    })

    const sendSubgroupObjectWithStatusBtn = document.getElementById('sendSubgroupObjectWithStatusBtn')
    sendSubgroupObjectWithStatusBtn.addEventListener('click', async () => {
      console.log('send subgroup stream object with status btn clicked')
      const trackAlias = form['object-track-alias'].value
      const objectStatus = Array.from(form['object-status']).filter((elem) => elem.checked)[0].value

      const objectPayloadArray = Uint8Array.from([])

      await client.sendSubgroupStreamObject(
        BigInt(trackAlias),
        groupId,
        subgroupId,
        objectId++,
        objectStatus,
        objectPayloadArray
      )
      objectIdElement.textContent = objectId
    })

    const ascendDatagramGroupId = document.getElementById('ascendDatagramGroupIdBtn')
    ascendDatagramGroupId.addEventListener('click', async () => {
      groupId++
      objectId = 0n
      console.log('ascend groupId', groupId)
      datagramGroupIdElement.textContent = groupId
      objectIdElement.textContent = objectId
    })

    const descendDatagramGroupId = document.getElementById('descendDatagramGroupIdBtn')
    descendDatagramGroupId.addEventListener('click', async () => {
      if (groupId === 0n) {
        return
      }
      groupId--
      objectId = 0n
      console.log('descend groupId', groupId)
      datagramGroupIdElement.textContent = groupId
      objectIdElement.textContent = objectId
    })

    const ascendSubgroupGroupId = document.getElementById('ascendSubgroupGroupIdBtn')
    ascendSubgroupGroupId.addEventListener('click', async () => {
      groupId++
      subgroupId = 0n
      objectId = 0n
      console.log('ascend groupId', groupId)
      subgroupGroupIdElement.textContent = groupId
      subgroupIdElement.textContent = subgroupId
      objectIdElement.textContent = objectId
    })

    const descendSubgroupGroupId = document.getElementById('descendSubgroupGroupIdBtn')
    descendSubgroupGroupId.addEventListener('click', async () => {
      if (groupId === 0n) {
        return
      }
      groupId--
      subgroupId = 0n
      objectId = 0n
      console.log('descend groupId', groupId)
      subgroupGroupIdElement.textContent = groupId
      subgroupIdElement.textContent = subgroupId
      objectIdElement.textContent = objectId
    })

    const ascendSubgroupId = document.getElementById('ascendSubgroupIdBtn')
    ascendSubgroupId.addEventListener('click', async () => {
      subgroupId++
      objectId = 0n
      console.log('ascend subgroupId', subgroupId)
      subgroupIdElement.textContent = subgroupId
      objectIdElement.textContent = objectId
    })

    const descendSubgroupId = document.getElementById('descendSubgroupIdBtn')
    descendSubgroupId.addEventListener('click', async () => {
      if (subgroupId === 0n) {
        return
      }
      subgroupId--
      objectId = 0n
      console.log('descend subgroupId', subgroupId)
      subgroupIdElement.textContent = subgroupId
      objectIdElement.textContent = objectId
    })

    await client.start()
  })

  const forwardingPreference = document.querySelectorAll('input[name="forwarding-preference"]')
  const subgroupHeaderContents = document.getElementById('subgroupHeaderContents')
  const datagramObjectContents = document.getElementById('datagramObjectContents')
  const sendDatagramObject = document.getElementById('sendDatagramObject')
  const sendDatagramObjectWithStatus = document.getElementById('sendDatagramObjectWithStatus')
  const sendSubgroupObject = document.getElementById('sendSubgroupObject')
  const sendSubgroupObjectWithStatus = document.getElementById('sendSubgroupObjectWithStatus')
  const headerField = document.getElementById('headerField')
  const objectField = document.getElementById('objectField')

  // change ui within track/subgroup
  forwardingPreference.forEach((elem) => {
    elem.addEventListener('change', async () => {
      if (elem.value === 'datagram') {
        datagramObjectContents.style.display = 'block'
        subgroupHeaderContents.style.display = 'none'
        sendDatagramObject.style.display = 'block'
        sendDatagramObjectWithStatus.style.display = 'block'
        sendSubgroupObject.style.display = 'none'
        sendSubgroupObjectWithStatus.style.display = 'none'
        headerField.style.display = 'none'
        objectField.style.display = 'none'
      } else if (elem.value === 'subgroup') {
        datagramObjectContents.style.display = 'none'
        subgroupHeaderContents.style.display = 'block'
        sendDatagramObject.style.display = 'none'
        sendDatagramObjectWithStatus.style.display = 'none'
        sendSubgroupObject.style.display = 'block'
        sendSubgroupObjectWithStatus.style.display = 'block'
        headerField.style.display = 'block'
        objectField.style.display = 'block'
      }
    })
  })
})
