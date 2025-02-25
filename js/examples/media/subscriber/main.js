import init, { MOQTClient } from '../../../pkg/moqt_client_sample'

const authInfo = 'secret'

function setupClientCallbacks(client) {
  client.onSetup(async (serverSetup) => {
    console.log({ serverSetup })
  })

  client.onSubgroupStreamHeader(async (subgroupStreamHeader) => {
    console.log({ subgroupStreamHeader })
  })

  client.onSubgroupStreamObject(async (subgroupStreamObject) => {
    console.log({ subgroupStreamObject })
  })
}

function sendSetupButtonClickHandler(client) {
  const sendSetupBtn = document.getElementById('sendSetupBtn')
  sendSetupBtn.addEventListener('click', async () => {
    const role = 2
    const versions = '0xff000008'.split(',').map(BigInt)
    const maxSubscribeId = BigInt(form['max-subscribe-id'].value)

    await client.sendSetupMessage(role, versions, maxSubscribeId)
  })
}

function sendSubscribeButtonClickHandler(client) {
  const sendSubscribeBtn = document.getElementById('sendSubscribeBtn')
  sendSubscribeBtn.addEventListener('click', async () => {
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
}

function setupButtonClickHandler(client) {
  sendSetupButtonClickHandler(client)
  sendSubscribeButtonClickHandler(client)
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
