import {
  MOQTClient,
  ObjectDatagramMessage,
  ObjectDatagramStatusMessage,
  PublishNamespaceMessage,
  RequestErrorMessage,
  ServerSetupMessage,
  SubgroupHeaderMessage,
  SubgroupObjectMessage,
  SubscribeOkMessage
} from '../../pkg/moqt_client_wasm'
import { MoqtClientWrapper } from '../../lib/moqt/moqtClient'

type HTMLFormControls = HTMLFormElement & {
  elements: HTMLFormControlsCollection
}

type FormFieldElement = HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement

interface TrackMetadata {
  trackNamespace: string[]
  trackName?: string
}

const moqtClient = new MoqtClientWrapper()
const subgroupHeaderSent = new Set<string>()
const requestTrackAliases = new Map<bigint, bigint>()
const requestTrackMetadata = new Map<bigint, TrackMetadata>()
let objectId = 0n
let groupId = 0n
let subgroupId = 0n

function getForm(): HTMLFormControls {
  const form = document.forms.namedItem('form')
  if (!form) {
    throw new Error('form element not found')
  }
  return form as HTMLFormControls
}

function getField(form: HTMLFormControls, name: string): FormFieldElement {
  const element = form.elements.namedItem(name)
  if (
    element instanceof HTMLInputElement ||
    element instanceof HTMLTextAreaElement ||
    element instanceof HTMLSelectElement
  ) {
    return element
  }

  const fallback = document.getElementById(name)
  if (
    fallback instanceof HTMLInputElement ||
    fallback instanceof HTMLTextAreaElement ||
    fallback instanceof HTMLSelectElement
  ) {
    return fallback
  }

  throw new Error(`field "${name}" not found`)
}

function getAliasSelect(form: HTMLFormControls): HTMLSelectElement {
  const field = getField(form, 'object-track-alias')
  if (!(field instanceof HTMLSelectElement)) {
    throw new Error('object-track-alias select not found')
  }
  return field
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

function parseTrackNamespace(value: string): string[] {
  return value
    .split('/')
    .map((part) => part.trim())
    .filter(Boolean)
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

function ensureRawClient(): MOQTClient {
  const client = moqtClient.getRawClient()
  if (!client) {
    throw new Error('MOQT client connection is not available')
  }
  return client
}

function setFieldValue(name: string, value: string): void {
  const form = getForm()
  getField(form, name).value = value
}

function buildAliasLabel(trackAlias: bigint, metadata?: TrackMetadata): string {
  if (!metadata) {
    return `${trackAlias.toString()}`
  }

  const namespace = metadata.trackNamespace.join('/')
  const trackPath = metadata.trackName ? `${namespace}/${metadata.trackName}` : namespace
  return `${trackAlias.toString()} :: ${trackPath}`
}

function registerTrackAlias(requestId: bigint, trackAlias: bigint, metadata?: TrackMetadata): void {
  const form = getForm()
  const aliasSelect = getAliasSelect(form)
  const aliasValue = trackAlias.toString()

  requestTrackAliases.set(requestId, trackAlias)
  if (metadata) {
    requestTrackMetadata.set(requestId, metadata)
  }

  let option = Array.from(aliasSelect.options).find((candidate) => candidate.value === aliasValue)
  if (!option) {
    option = document.createElement('option')
    option.value = aliasValue
    aliasSelect.appendChild(option)
  }
  option.textContent = buildAliasLabel(trackAlias, metadata ?? requestTrackMetadata.get(requestId))
  aliasSelect.value = aliasValue
  setFieldValue('subscribe-assigned-track-alias', aliasValue)
  setFieldValue('unsubscribe-subscribe-id', requestId.toString())
}

function pruneAliasState(trackAlias: bigint): void {
  const prefix = `${trackAlias.toString()}:`
  for (const key of subgroupHeaderSent) {
    if (key.startsWith(prefix)) {
      subgroupHeaderSent.delete(key)
    }
  }
}

function unregisterTrackAliasByRequestId(requestId: bigint): void {
  const form = getForm()
  const aliasSelect = getAliasSelect(form)
  const trackAlias = requestTrackAliases.get(requestId)

  requestTrackAliases.delete(requestId)
  requestTrackMetadata.delete(requestId)

  if (trackAlias === undefined) {
    return
  }

  const option = Array.from(aliasSelect.options).find((candidate) => candidate.value === trackAlias.toString())
  if (option) {
    option.remove()
  }

  pruneAliasState(trackAlias)
  moqtClient.clearSubgroupObjectHandler(trackAlias)

  const nextValue = aliasSelect.options[0]?.value ?? ''
  aliasSelect.value = nextValue
  setFieldValue('subscribe-assigned-track-alias', nextValue)
}

function getSelectedTrackAlias(form: HTMLFormControls): bigint {
  const aliasValue = getAliasSelect(form).value
  if (!aliasValue) {
    throw new Error('No active track alias selected')
  }
  return BigInt(aliasValue)
}

function attachSubgroupObjectHandler(trackAlias: bigint, receivedTextElement: HTMLElement): void {
  moqtClient.setOnSubgroupObjectHandler(trackAlias, (receivedGroupId, subgroupObject: SubgroupObjectMessage) => {
    console.info({ groupId: receivedGroupId, subgroupObject })
    if (subgroupObject.objectPayload.length > 0) {
      describeReceivedObject(subgroupObject.objectPayload, receivedTextElement)
    }
  })
}

function isRequestError(response: SubscribeOkMessage | RequestErrorMessage): response is RequestErrorMessage {
  return 'errorCode' in response
}

function handleServerSetup(serverSetup: ServerSetupMessage): void {
  console.info({ serverSetup })
}

function handlePublishNamespace(publishNamespace: PublishNamespaceMessage): void {
  console.info({ publishNamespace })
}

function handlePublishNamespaceResponse(response: RequestErrorMessage | { requestId: bigint }): void {
  console.info({ publishNamespaceResponse: response })
}

function handleSubscribeNamespaceResponse(response: RequestErrorMessage | { requestId: bigint }): void {
  console.info({ subscribeNamespaceResponse: response })
}

function handleObjectDatagram(message: ObjectDatagramMessage, receivedTextElement: HTMLElement): void {
  console.info({ objectDatagram: message })
  if (message.objectPayload.length > 0) {
    describeReceivedObject(message.objectPayload, receivedTextElement)
  }
}

function handleObjectDatagramStatus(message: ObjectDatagramStatusMessage): void {
  console.info({ objectDatagramStatus: message })
}

function handleSubgroupHeader(message: SubgroupHeaderMessage): void {
  console.info({ subgroupHeader: message })
}

function registerClientCallbacks(receivedTextElement: HTMLElement): void {
  moqtClient.setOnServerSetupHandler(handleServerSetup)
  moqtClient.setOnPublishNamespaceHandler(async ({ publishNamespace, respondOk }) => {
    handlePublishNamespace(publishNamespace)
    await respondOk()
  })
  moqtClient.setOnPublishNamespaceResponseHandler(handlePublishNamespaceResponse)
  moqtClient.setOnSubscribeNamespaceResponseHandler(handleSubscribeNamespaceResponse)
  moqtClient.setOnSubscribeResponseHandler((response) => {
    console.info({ subscribeResponse: response })
    if (isRequestError(response)) {
      requestTrackMetadata.delete(response.requestId)
      return
    }

    registerTrackAlias(response.requestId, response.trackAlias, requestTrackMetadata.get(response.requestId))
    attachSubgroupObjectHandler(response.trackAlias, receivedTextElement)
  })
  moqtClient.setOnIncomingSubscribeHandler(async ({ subscribe, isSuccess, code, respondOk, respondError }) => {
    console.info({ subscribeMessage: subscribe, isSuccess, code })
    if (!isSuccess) {
      await respondError(BigInt(code), 'subscribe rejected')
      return
    }

    const trackAlias = await respondOk(0n)
    registerTrackAlias(subscribe.requestId, trackAlias, {
      trackNamespace: [...subscribe.trackNamespace],
      trackName: subscribe.trackName
    })
  })
  moqtClient.setOnIncomingUnsubscribeHandler((requestId) => {
    console.info({ unsubscribeMessage: { requestId } })
    unregisterTrackAliasByRequestId(requestId)
  })
  moqtClient.setOnObjectDatagramHandler((message) => handleObjectDatagram(message, receivedTextElement))
  moqtClient.setOnObjectDatagramStatusHandler(handleObjectDatagramStatus)
  moqtClient.setOnSubgroupHeaderHandler(handleSubgroupHeader)
  moqtClient.setOnConnectionClosedHandler(() => {
    console.info('connection closed')
    subgroupHeaderSent.clear()
    requestTrackAliases.clear()
    requestTrackMetadata.clear()
    moqtClient.clearSubgroupObjectHandlers()
    setFieldValue('subscribe-assigned-track-alias', '')
    setFieldValue('unsubscribe-subscribe-id', '')
    getAliasSelect(getForm()).innerHTML = ''
  })
}

function setupConnectButton(): void {
  const connectBtn = document.getElementById('connectBtn')
  if (!(connectBtn instanceof HTMLButtonElement)) {
    return
  }

  connectBtn.addEventListener('click', async () => {
    const form = getForm()
    const url = getField(form, 'url').value
    const receivedTextElement = document.getElementById('received-text')
    if (!receivedTextElement) {
      return
    }

    await moqtClient.connect(url, { sendSetup: false })
    registerClientCallbacks(receivedTextElement)
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
    requestTrackAliases.clear()
    requestTrackMetadata.clear()
    objectId = 0n
    groupId = 0n
    subgroupId = 0n
  })
}

function setupActionButtons(): void {
  const objectIdElement = document.getElementById('objectId')
  const datagramGroupIdElement = document.getElementById('datagramGroupId')
  const subgroupGroupIdElement = document.getElementById('subgroupGroupId')
  const subgroupIdElement = document.getElementById('subgroupId')

  const sendSetupBtn = document.getElementById('sendSetupBtn') as HTMLButtonElement | null
  sendSetupBtn?.addEventListener('click', async () => {
    const form = getForm()
    const versionsInput = getField(form, 'versions').value
    const maxRequestId = BigInt(getField(form, 'max-subscribe-id').value)
    await moqtClient.sendClientSetup(toBigUint64Array(versionsInput), maxRequestId)
  })

  const sendPublishNamespaceBtn = document.getElementById('sendPublishNamespaceBtn') as HTMLButtonElement | null
  sendPublishNamespaceBtn?.addEventListener('click', async () => {
    const form = getForm()
    const trackNamespace = parseTrackNamespace(getField(form, 'publish-track-namespace').value)
    const authInfo = getField(form, 'auth-info').value
    await moqtClient.publishNamespace(trackNamespace, authInfo)
  })

  const sendSubscribeNamespaceBtn = document.getElementById('sendSubscribeNamespaceBtn') as HTMLButtonElement | null
  sendSubscribeNamespaceBtn?.addEventListener('click', async () => {
    const form = getForm()
    const trackNamespacePrefix = parseTrackNamespace(getField(form, 'track-namespace-prefix').value)
    const authInfo = getField(form, 'auth-info').value
    await moqtClient.subscribeNamespace(trackNamespacePrefix, authInfo)
  })

  const sendSubscribeBtn = document.getElementById('sendSubscribeBtn') as HTMLButtonElement | null
  sendSubscribeBtn?.addEventListener('click', async () => {
    const form = getForm()
    const requestId = BigInt(getField(form, 'subscribe-request-id').value)
    const trackNamespace = parseTrackNamespace(getField(form, 'subscribe-track-namespace').value)
    const trackName = getField(form, 'track-name').value
    const authInfo = getField(form, 'auth-info').value
    const subscriberPriority = Number(getField(form, 'subscriber-priority').value)
    const groupOrder = Number(getRadioValue(form, 'group-order'))
    const filterType = Number(getRadioValue(form, 'filter-type'))
    const startGroup = BigInt(getField(form, 'start-group').value)
    const startObject = BigInt(getField(form, 'start-object').value)
    const endGroup = BigInt(getField(form, 'end-group').value)
    const forward = getRadioValue(form, 'forwarding') === 'true'

    requestTrackMetadata.set(requestId, {
      trackNamespace: [...trackNamespace],
      trackName
    })

    const receivedTextElement = document.getElementById('received-text')
    if (!receivedTextElement) {
      return
    }

    const trackAlias = await moqtClient.subscribe(requestId, trackNamespace, trackName, authInfo, {
      subscriberPriority,
      groupOrder,
      filterType,
      startGroup,
      startObject,
      endGroup,
      forward
    })

    registerTrackAlias(requestId, trackAlias, {
      trackNamespace: [...trackNamespace],
      trackName
    })
    attachSubgroupObjectHandler(trackAlias, receivedTextElement)
  })

  const sendUnsubscribeBtn = document.getElementById('sendUnsubscribeBtn') as HTMLButtonElement | null
  sendUnsubscribeBtn?.addEventListener('click', async () => {
    const form = getForm()
    const requestId = BigInt(getField(form, 'unsubscribe-subscribe-id').value)
    await moqtClient.unsubscribe(requestId)
    unregisterTrackAliasByRequestId(requestId)
  })

  const sendDatagramObjectBtn = document.getElementById('sendDatagramObjectBtn') as HTMLButtonElement | null
  sendDatagramObjectBtn?.addEventListener('click', async () => {
    const form = getForm()
    const client = ensureRawClient()
    const trackAlias = getSelectedTrackAlias(form)
    const publisherPriority = Number(getField(form, 'publisher-priority').value)
    const objectPayloadString = getField(form, 'payload').value
    const objectPayloadArray = new TextEncoder().encode(objectPayloadString)

    await client.sendObjectDatagram(trackAlias, groupId, objectId++, publisherPriority, objectPayloadArray, undefined)
    if (objectIdElement) {
      objectIdElement.textContent = objectId.toString()
    }
  })

  const sendDatagramObjectWithStatusBtn = document.getElementById(
    'sendDatagramObjectWithStatusBtn'
  ) as HTMLButtonElement | null
  sendDatagramObjectWithStatusBtn?.addEventListener('click', async () => {
    const form = getForm()
    const client = ensureRawClient()
    const trackAlias = getSelectedTrackAlias(form)
    const publisherPriority = Number(getField(form, 'publisher-priority').value)
    const objectStatus = Number(getRadioValue(form, 'object-status'))

    await client.sendObjectDatagramStatus(trackAlias, groupId, objectId++, publisherPriority, objectStatus, undefined)
    if (objectIdElement) {
      objectIdElement.textContent = objectId.toString()
    }
  })

  const sendSubgroupObjectBtn = document.getElementById('sendSubgroupObjectBtn') as HTMLButtonElement | null
  sendSubgroupObjectBtn?.addEventListener('click', async () => {
    const form = getForm()
    const client = ensureRawClient()
    const trackAlias = getSelectedTrackAlias(form)
    const publisherPriority = Number(getField(form, 'publisher-priority').value)
    const objectPayloadString = getField(form, 'payload').value
    const objectPayloadArray = new TextEncoder().encode(objectPayloadString)
    const key = `${trackAlias.toString()}:${groupId.toString()}:${subgroupId.toString()}`

    if (!subgroupHeaderSent.has(key)) {
      await client.sendSubgroupHeader(trackAlias, groupId, subgroupId, publisherPriority)
      subgroupHeaderSent.add(key)
    }

    await client.sendSubgroupObject(
      trackAlias,
      groupId,
      subgroupId,
      objectId++,
      undefined,
      objectPayloadArray,
      undefined
    )
    if (objectIdElement) {
      objectIdElement.textContent = objectId.toString()
    }
  })

  const sendSubgroupObjectWithStatusBtn = document.getElementById(
    'sendSubgroupObjectWithStatusBtn'
  ) as HTMLButtonElement | null
  sendSubgroupObjectWithStatusBtn?.addEventListener('click', async () => {
    const form = getForm()
    const client = ensureRawClient()
    const trackAlias = getSelectedTrackAlias(form)
    const publisherPriority = Number(getField(form, 'publisher-priority').value)
    const objectStatus = Number(getRadioValue(form, 'object-status'))
    const key = `${trackAlias.toString()}:${groupId.toString()}:${subgroupId.toString()}`

    if (!subgroupHeaderSent.has(key)) {
      await client.sendSubgroupHeader(trackAlias, groupId, subgroupId, publisherPriority)
      subgroupHeaderSent.add(key)
    }

    await client.sendSubgroupObject(
      trackAlias,
      groupId,
      subgroupId,
      objectId++,
      objectStatus,
      new Uint8Array(),
      undefined
    )
    if (objectIdElement) {
      objectIdElement.textContent = objectId.toString()
    }
  })

  const ascendDatagramGroupId = document.getElementById('ascendDatagramGroupIdBtn') as HTMLButtonElement | null
  ascendDatagramGroupId?.addEventListener('click', () => {
    groupId += 1n
    objectId = 0n
    if (datagramGroupIdElement) {
      datagramGroupIdElement.textContent = groupId.toString()
    }
    if (objectIdElement) {
      objectIdElement.textContent = objectId.toString()
    }
  })

  const descendDatagramGroupId = document.getElementById('descendDatagramGroupIdBtn') as HTMLButtonElement | null
  descendDatagramGroupId?.addEventListener('click', () => {
    if (groupId === 0n) {
      return
    }
    groupId -= 1n
    objectId = 0n
    if (datagramGroupIdElement) {
      datagramGroupIdElement.textContent = groupId.toString()
    }
    if (objectIdElement) {
      objectIdElement.textContent = objectId.toString()
    }
  })

  const ascendSubgroupGroupId = document.getElementById('ascendSubgroupGroupIdBtn') as HTMLButtonElement | null
  ascendSubgroupGroupId?.addEventListener('click', () => {
    groupId += 1n
    subgroupId = 0n
    objectId = 0n
    if (subgroupGroupIdElement) {
      subgroupGroupIdElement.textContent = groupId.toString()
    }
    if (subgroupIdElement) {
      subgroupIdElement.textContent = subgroupId.toString()
    }
    if (objectIdElement) {
      objectIdElement.textContent = objectId.toString()
    }
  })

  const descendSubgroupGroupId = document.getElementById('descendSubgroupGroupIdBtn') as HTMLButtonElement | null
  descendSubgroupGroupId?.addEventListener('click', () => {
    if (groupId === 0n) {
      return
    }
    groupId -= 1n
    subgroupId = 0n
    objectId = 0n
    if (subgroupGroupIdElement) {
      subgroupGroupIdElement.textContent = groupId.toString()
    }
    if (subgroupIdElement) {
      subgroupIdElement.textContent = subgroupId.toString()
    }
    if (objectIdElement) {
      objectIdElement.textContent = objectId.toString()
    }
  })

  const ascendSubgroupId = document.getElementById('ascendSubgroupIdBtn') as HTMLButtonElement | null
  ascendSubgroupId?.addEventListener('click', () => {
    subgroupId += 1n
    objectId = 0n
    if (subgroupIdElement) {
      subgroupIdElement.textContent = subgroupId.toString()
    }
    if (objectIdElement) {
      objectIdElement.textContent = objectId.toString()
    }
  })

  const descendSubgroupId = document.getElementById('descendSubgroupIdBtn') as HTMLButtonElement | null
  descendSubgroupId?.addEventListener('click', () => {
    if (subgroupId === 0n) {
      return
    }
    subgroupId -= 1n
    objectId = 0n
    if (subgroupIdElement) {
      subgroupIdElement.textContent = subgroupId.toString()
    }
    if (objectIdElement) {
      objectIdElement.textContent = objectId.toString()
    }
  })
}

setupConnectButton()
setupCloseButton()
setupActionButtons()
