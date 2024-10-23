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
    })

    client.onSubscribe(async (subscribeMessage, isSuccess, code) => {
      console.log({ subscribeMessage })
      if (isSuccess) {
        let expire = 0n
        subscribeId = BigInt(subscribeMessage.subscribe_id)
        trackAlias = BigInt(subscribeMessage.track_alias)

        console.log('subscribeId', subscribeId, 'trackAlias', trackAlias)
        let subscribeId = BigInt(subscribeMessage.subscribe_id)

        await client.sendSubscribeOkMessage(subscribeId, expire, authInfo)
      } else {
        // TODO: send subscribe error
      }
    })

    client.onSubscribeResponse(async (subscribeResponse) => {
      console.log({ subscribeResponse })
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
      const commonCatalogFormat = form['common-catalog-format'].value
      const isAddPath = !!form['add-path'].checked

      console.log({ streamDatagram, messageType, versions, role, isAddPath })

      switch (messageType) {
        case 'setup':
          let maxSubscribeId = 5n
          await client.sendSetupMessage(role, versions, maxSubscribeId)
          break
        case 'announce':
          await client.sendAnnounceMessage(trackNamespace, 1, authInfo)
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
        case 'object-catalog':
          await client.sendObjectStreamTrack(subscribeId, groupId, objectId++, objectPayload)
          break
        case 'object-track':
          if (!headerSend) {
            await client.sendStreamHeaderTrackMessage(subscribeId, trackAlias, 0)
            headerSend = true
          }
          let groupId = 0n
          let objectPayload = new Uint8Array([0xde, 0xad, 0xbe, 0xef])
          // let objectPayload = new Uint8Array([0x00, 0x01, 0x02, 0x03])
          await client.sendObjectStreamTrack(subscribeId, groupId, objectId++, objectPayload)
          break
      }
    }

    sendBtn.addEventListener('click', send)

    await client.start()
  })
})
