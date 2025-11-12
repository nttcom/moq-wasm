import { MOQTClient } from '../../pkg/moqt_client_sample'
import { MoqtClientWrapper } from '../../lib/moqt/moqtClient'

type HTMLFormControls = HTMLFormElement & {
  elements: HTMLFormControlsCollection
}

const moqtClient = new MoqtClientWrapper()

const subgroupHeaderSent = new Set<string>()
let objectId = 0n
let groupId = 0n
let subgroupId = 0n

function getForm(): HTMLFormElement {
  const form = document.forms.namedItem('form')
  if (!form) {
    throw new Error('form element not found')
  }
  return form
}

type TextInputElement = HTMLInputElement | HTMLTextAreaElement

function getField(form: HTMLFormControls, name: string): TextInputElement {
  const element = form.elements.namedItem(name)
  if (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement) {
    return element
  }

  const fallback = document.getElementById(name)
  if (fallback instanceof HTMLInputElement || fallback instanceof HTMLTextAreaElement) {
    return fallback
  }

  throw new Error(`input "${name}" not found`)
}

function getRadioValue(form: HTMLFormControls, name: string): string {
  const elements = form.elements.namedItem(name)
  if (!elements) {
    return ''
  }

  if (elements instanceof RadioNodeList) {
    return elements.value
  }

  if (elements instanceof HTMLInputElement && elements.type === 'radio') {
    return elements.checked ? elements.value : ''
  }

  throw new Error(`radio group "${name}" not found`)
}

function toBigUint64Array(input: string): BigUint64Array {
  const values = input
    .split(',')
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
    .map((part) => BigInt(part))
  return new BigUint64Array(values)
}

function describeReceivedObject(payload: ArrayBuffer | Uint8Array, container: HTMLElement): void {
  const brElement = document.createElement('br')
  container.prepend(brElement)

  const receivedArray = payload instanceof Uint8Array ? payload : new Uint8Array(payload)
  const receivedText = new TextDecoder().decode(receivedArray)

  const receivedElement = document.createElement('p')
  receivedElement.textContent = receivedText
  container.prepend(receivedElement)
}

function setupConnectButton(): void {
  const connectBtn = document.getElementById('connectBtn')
  if (!(connectBtn instanceof HTMLButtonElement)) {
    return
  }

  connectBtn.addEventListener('click', async () => {
    const form = getForm() as HTMLFormControls
    const url = getField(form, 'url').value
    const receivedTextElement = document.getElementById('received-text')
    if (!receivedTextElement) {
      return
    }

    await moqtClient.connect(url, { sendSetup: false })
    const client = moqtClient.getRawClient()
    if (!client) {
      console.error('MOQT client connection is not available')
      return
    }

    moqtClient.setOnServerSetupHandler((serverSetup) => {
      console.log({ serverSetup })
    })

    moqtClient.setOnAnnounceHandler(async (announceMessage) => {
      console.log({ announceMessage })
      await client.sendAnnounceOkMessage(announceMessage.trackNamespace)
    })

    moqtClient.setOnSubscribeResponseHandler((subscribeResponse) => {
      console.log({ subscribeResponse })
    })

    moqtClient.setOnIncomingSubscribeHandler(async ({ subscribe, isSuccess, code, respondOk, respondError }) => {
      console.log({ subscribeMessage: subscribe })

      const authInfo = getField(form, 'auth-info').value
      const forwardingPreference = getRadioValue(form, 'forwarding-preference')

      if (isSuccess) {
        const expire = 0n
        await respondOk(expire, authInfo, forwardingPreference)
      } else {
        const reasonPhrase = 'subscribe error'
        await respondError(BigInt(code), reasonPhrase)
      }
    })

    client.onAnnounceResponse((announceResponseMessage: any) => {
      console.log({ announceResponseMessage })
    })

    client.onSubscribeAnnouncesResponse((subscribeAnnouncesResponse: any) => {
      console.log({ subscribeAnnouncesResponse })
    })

    client.onUnsubscribe((unsubscribeMessage: any) => {
      console.log({ unsubscribeMessage })
    })

    client.onDatagramObject((datagramObject: any) => {
      console.log({ datagramObject })
      const payload = datagramObject.objectPayload ?? datagramObject.object_payload
      if (payload) {
        describeReceivedObject(payload, receivedTextElement)
      }
    })

    client.onDatagramObjectStatus((datagramObjectStatus: any) => {
      console.log({ datagramObjectStatus })
    })

    client.onSubgroupStreamHeader((subgroupStreamHeader: any) => {
      console.log({ subgroupStreamHeader })
    })

    setupActionButtons(client, form, receivedTextElement)
  })
}

function setupCloseButton(): void {
  const closeBtn = document.getElementById('closeBtn')
  if (!(closeBtn instanceof HTMLButtonElement)) {
    return
  }

  closeBtn.addEventListener('click', async () => {
    await moqtClient.disconnect()
    subgroupHeaderSent.clear()
    objectId = 0n
    groupId = 0n
    subgroupId = 0n
  })
}

function setupActionButtons(client: MOQTClient, form: HTMLFormControls, receivedTextElement: HTMLElement): void {
  const objectIdElement = document.getElementById('objectId')
  const datagramGroupIdElement = document.getElementById('datagramGroupId')
  const subgroupGroupIdElement = document.getElementById('subgroupGroupId')
  const subgroupIdElement = document.getElementById('subgroupId')

  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement | null
  sendSetupBtn?.addEventListener('click', async () => {
    console.log('send setup btn clicked')
    const versionsInput = getField(form, 'versions').value
    const maxSubscribeId = BigInt(getField(form, 'max-subscribe-id').value)
    await moqtClient.sendSetupMessage(toBigUint64Array(versionsInput), maxSubscribeId)
  })

  const sendAnnounceBtn = document.getElementById('sendAnnounceBtn') as HTMLButtonElement | null
  sendAnnounceBtn?.addEventListener('click', async () => {
    console.log('send announce btn clicked')
    const trackNamespace = getField(form, 'announce-track-namespace').value.split('/').filter(Boolean)
    const authInfo = getField(form, 'auth-info').value

    await moqtClient.announce(trackNamespace, authInfo)
  })

  const sendSubscribeAnnouncesBtn = document.getElementById('sendSubscribeAnnouncesBtn') as HTMLButtonElement | null
  sendSubscribeAnnouncesBtn?.addEventListener('click', async () => {
    console.log('send subscribe announces btn clicked')
    const trackNamespacePrefix = getField(form, 'track-namespace-prefix').value.split('/').filter(Boolean)
    const authInfo = getField(form, 'auth-info').value

    await moqtClient.subscribeAnnounces(trackNamespacePrefix, authInfo)
  })

  const sendSubscribeBtn = document.getElementById('sendSubscribeBtn') as HTMLButtonElement | null
  sendSubscribeBtn?.addEventListener('click', async () => {
    console.log('send subscribe btn clicked')
    const subscribeId = BigInt(getField(form, 'subscribe-subscribe-id').value)
    const trackAlias = BigInt(getField(form, 'subscribe-track-alias').value)
    const trackNamespace = getField(form, 'subscribe-track-namespace').value.split('/').filter(Boolean)
    const trackName = getField(form, 'track-name').value
    const subscriberPriority = getField(form, 'subscriber-priority').value
    const groupOrder = getRadioValue(form, 'group-order')
    const filterType = getRadioValue(form, 'filter-type')
    const startGroup = BigInt(getField(form, 'start-group').value)
    const startObject = BigInt(getField(form, 'start-object').value)
    const endGroup = BigInt(getField(form, 'end-group').value)
    const authInfo = getField(form, 'auth-info').value

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

    moqtClient.setOnSubgroupObjectHandler(trackAlias, (receivedGroupId, subgroupStreamObject) => {
      console.log({ groupId: receivedGroupId, subgroupStreamObject })
      const fallbackPayload = (subgroupStreamObject as { object_payload?: Uint8Array }).object_payload
      const objectPayload = subgroupStreamObject.objectPayload ?? fallbackPayload
      if (objectPayload && objectPayload.length > 0) {
        describeReceivedObject(objectPayload, receivedTextElement)
      }
    })

    await client.sendSubscribeMessage(
      subscribeId,
      trackAlias,
      trackNamespace,
      trackName,
      Number(subscriberPriority),
      Number(groupOrder),
      Number(filterType),
      startGroup,
      startObject,
      endGroup,
      authInfo
    )
  })

  const sendUnsubscribeBtn = document.getElementById('sendUnsubscribeBtn') as HTMLButtonElement | null
  sendUnsubscribeBtn?.addEventListener('click', async () => {
    console.log('send unsubscribe btn clicked')
    const subscribeId = getField(form, 'unsubscribe-subscribe-id').value
    await client.sendUnsubscribeMessage(BigInt(subscribeId))
  })

  const sendDatagramObjectBtn = document.getElementById('sendDatagramObjectBtn') as HTMLButtonElement | null
  sendDatagramObjectBtn?.addEventListener('click', async () => {
    console.log('send datagram object btn clicked')
    const trackAlias = BigInt(getField(form, 'object-track-alias').value)
    const publisherPriority = Number(getField(form, 'publisher-priority').value)
    const objectPayloadString = getField(form, 'object-payload').value
    const objectPayloadArray = new TextEncoder().encode(objectPayloadString)

    await client.sendDatagramObject(trackAlias, groupId, objectId++, publisherPriority, objectPayloadArray)
    objectIdElement && (objectIdElement.textContent = objectId.toString())
  })

  const sendDatagramObjectWithStatusBtn = document.getElementById(
    'sendDatagramObjectWithStatusBtn'
  ) as HTMLButtonElement | null
  sendDatagramObjectWithStatusBtn?.addEventListener('click', async () => {
    console.log('send datagram object with status btn clicked')
    const trackAlias = BigInt(getField(form, 'object-track-alias').value)
    const publisherPriority = Number(getField(form, 'publisher-priority').value)
    const objectStatus = Number(getRadioValue(form, 'object-status'))

    await client.sendDatagramObjectStatus(trackAlias, groupId, objectId++, publisherPriority, objectStatus)
    objectIdElement && (objectIdElement.textContent = objectId.toString())
  })

  const sendSubgroupObjectBtn = document.getElementById('sendSubgroupObjectBtn') as HTMLButtonElement | null
  sendSubgroupObjectBtn?.addEventListener('click', async () => {
    console.log('send subgroup stream object btn clicked')
    const trackAlias = BigInt(getField(form, 'object-track-alias').value)
    const publisherPriority = Number(getField(form, 'publisher-priority').value)
    const objectPayloadString = getField(form, 'object-payload').value
    const objectPayloadArray = new TextEncoder().encode(objectPayloadString)
    const key = `${groupId}:${subgroupId}`

    if (!subgroupHeaderSent.has(key)) {
      await client.sendSubgroupStreamHeaderMessage(trackAlias, groupId, subgroupId, publisherPriority)
      subgroupHeaderSent.add(key)
    }

    await client.sendSubgroupStreamObject(trackAlias, groupId, subgroupId, objectId++, undefined, objectPayloadArray)
    objectIdElement && (objectIdElement.textContent = objectId.toString())
  })

  const sendSubgroupObjectWithStatusBtn = document.getElementById(
    'sendSubgroupObjectWithStatusBtn'
  ) as HTMLButtonElement | null
  sendSubgroupObjectWithStatusBtn?.addEventListener('click', async () => {
    console.log('send subgroup stream object with status btn clicked')
    const trackAlias = BigInt(getField(form, 'object-track-alias').value)
    const publisherPriority = Number(getField(form, 'publisher-priority').value)
    const objectStatus = Number(getRadioValue(form, 'object-status'))
    const objectPayloadArray = new Uint8Array()
    const key = `${groupId}:${subgroupId}`

    if (!subgroupHeaderSent.has(key)) {
      await client.sendSubgroupStreamHeaderMessage(trackAlias, groupId, subgroupId, publisherPriority)
      subgroupHeaderSent.add(key)
    }

    await client.sendSubgroupStreamObject(trackAlias, groupId, subgroupId, objectId++, objectStatus, objectPayloadArray)
    objectIdElement && (objectIdElement.textContent = objectId.toString())
  })

  const ascendDatagramGroupId = document.getElementById('ascendDatagramGroupIdBtn') as HTMLButtonElement | null
  ascendDatagramGroupId?.addEventListener('click', () => {
    groupId++
    objectId = 0n
    datagramGroupIdElement && (datagramGroupIdElement.textContent = groupId.toString())
    objectIdElement && (objectIdElement.textContent = objectId.toString())
  })

  const descendDatagramGroupId = document.getElementById('descendDatagramGroupIdBtn') as HTMLButtonElement | null
  descendDatagramGroupId?.addEventListener('click', () => {
    if (groupId === 0n) {
      return
    }
    groupId--
    objectId = 0n
    datagramGroupIdElement && (datagramGroupIdElement.textContent = groupId.toString())
    objectIdElement && (objectIdElement.textContent = objectId.toString())
  })

  const ascendSubgroupGroupId = document.getElementById('ascendSubgroupGroupIdBtn') as HTMLButtonElement | null
  ascendSubgroupGroupId?.addEventListener('click', () => {
    groupId++
    subgroupId = 0n
    objectId = 0n
    subgroupGroupIdElement && (subgroupGroupIdElement.textContent = groupId.toString())
    subgroupIdElement && (subgroupIdElement.textContent = subgroupId.toString())
    objectIdElement && (objectIdElement.textContent = objectId.toString())
  })

  const descendSubgroupGroupId = document.getElementById('descendSubgroupGroupIdBtn') as HTMLButtonElement | null
  descendSubgroupGroupId?.addEventListener('click', () => {
    if (groupId === 0n) {
      return
    }
    groupId--
    subgroupId = 0n
    objectId = 0n
    subgroupGroupIdElement && (subgroupGroupIdElement.textContent = groupId.toString())
    subgroupIdElement && (subgroupIdElement.textContent = subgroupId.toString())
    objectIdElement && (objectIdElement.textContent = objectId.toString())
  })

  const ascendSubgroupId = document.getElementById('ascendSubgroupIdBtn') as HTMLButtonElement | null
  ascendSubgroupId?.addEventListener('click', () => {
    subgroupId++
    objectId = 0n
    subgroupIdElement && (subgroupIdElement.textContent = subgroupId.toString())
    objectIdElement && (objectIdElement.textContent = objectId.toString())
  })

  const descendSubgroupId = document.getElementById('descendSubgroupIdBtn') as HTMLButtonElement | null
  descendSubgroupId?.addEventListener('click', () => {
    if (subgroupId === 0n) {
      return
    }
    subgroupId--
    objectId = 0n
    subgroupIdElement && (subgroupIdElement.textContent = subgroupId.toString())
    objectIdElement && (objectIdElement.textContent = objectId.toString())
  })
}

setupConnectButton()
setupCloseButton()
