import init, { MOQTClient } from './pkg/moqt_client_sample'

// TODO: impl close
init().then(async () => {
  console.log('init wasm-pack')

  let subscribeId
  let trackAlias
  let headerSend = false
  let objectId = 0n

  const connectBtn = document.getElementById('connectBtn')
  connectBtn.addEventListener('click', async () => {
    const url = document.form.url.value
    const authInfo = form['auth-info'].value

    const client = new MOQTClient(url)
    console.log(client.id, client)
    console.log('URL:', client.url())

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
      let subscribeId = BigInt(subscribeMessage.subscribe_id)
      if (isSuccess) {
        let expire = 0n
        subscribeId = BigInt(subscribeMessage.subscribe_id)
        trackAlias = BigInt(subscribeMessage.track_alias)

        console.log('subscribeId', subscribeId, 'trackAlias', trackAlias)

        await client.sendSubscribeOkMessage(subscribeId, expire, authInfo)
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

    client.onStreamHeaderTrack(async (streamHeaderTrack) => {
      console.log({ streamHeaderTrack })
    })

    client.onObjectStreamTrack(async (objectStreamTrack) => {
      console.log({ objectStreamTrack })
    })

    client.onStreamHeaderSubgroup(async (streamHeaderSubgroup) => {
      console.log({ streamHeaderSubgroup })
    })

    client.onObjectStreamSubgroup(async (objectStreamSubgroup) => {
      console.log({ objectStreamSubgroup })
    })

    const sendBtn = document.getElementById('sendBtn')

    const send = async () => {
      console.log('send btn clicked')
      const streamDatagram = Array.from(form['stream-datagram']).filter((elem) => elem.checked)[0].value
      const messageType = form['message-type'].value
      const trackNamespace = form['track-namespace'].value.split('/')
      const trackName = form['track-name'].value
      const authInfo = form['auth-info'].value
      const versions = form['versions'].value.split(',').map(BigInt)
      const role = Array.from(form['role']).filter((elem) => elem.checked)[0].value
      const isAddPath = !!form['add-path'].checked
      let objectPayload

      console.log({ streamDatagram, messageType, versions, role, isAddPath })

      switch (messageType) {
        case 'setup':
          let maxSubscribeId = 5n
          await client.sendSetupMessage(role, versions, maxSubscribeId)
          break
        case 'announce':
          await client.sendAnnounceMessage(trackNamespace, authInfo)
          break
        case 'unannounce':
          await client.sendUnannounceMessage(trackNamespace)
          break
        case 'subscribe':
          await client.sendSubscribeMessage(trackNamespace, trackName, authInfo)
          break
        case 'unsubscribe':
          await client.sendUnsubscribeMessage(trackNamespace, trackName)
          break
        case 'subscribe-namespace':
          await client.sendSubscribeNamespaceMessage(trackNamespace, authInfo)
          break
        case 'object-track':
          if (!headerSend) {
            await client.sendStreamHeaderTrackMessage(subscribeId, trackAlias, 0)
            headerSend = true
          }
          let groupId = 0n
<<<<<<< HEAD
          objectPayload = new Uint8Array([0xde, 0xad, 0xbe, 0xef])
          // let objectPayload = new Uint8Array([0x00, 0x01, 0x02, 0x03])
=======
          let objectPayload = new Uint8Array([0xde, 0xad, 0xbe, 0xef])
>>>>>>> draft-06
          await client.sendObjectStreamTrack(subscribeId, groupId, objectId++, objectPayload)
          break
        case 'object-subgroup':
          if (!headerSend) {
            let groupId = 0n
            let subgroupId = 0n
            await client.sendStreamHeaderSubgroupMessage(subscribeId, trackAlias, groupId, subgroupId, 0)
            headerSend = true
          }

          objectPayload = new Uint8Array([0xde, 0xad, 0xbe, 0xef])
          await client.sendObjectStreamSubgroup(subscribeId, objectId++, objectPayload)
          break
      }
    }

    sendBtn.addEventListener('click', send)

    await client.start()
  })
})
