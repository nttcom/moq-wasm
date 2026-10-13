import { MOQTClient, RequestErrorMessage, SubgroupObjectMessage, SubscribeOkMessage } from '../../pkg/moqt'
import { FilterType, GroupOrder, MoqtClientWrapper } from '../../lib/moqt/moqtClient'
import { setUpRelayUrlControls } from '../../utils/relayPresets'
import { requireRawClient } from '../media/common'
import { element, parseTrackNamespace, buildDefaultTrackNamespace } from '../../utils/page'

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

const form = element<HTMLFormElement>('form')
const aliasSelect = element<HTMLSelectElement>('object-track-alias')
const receivedTextElement = element('received-text')

function field(name: string): HTMLInputElement {
  return form.elements.namedItem(name) as HTMLInputElement
}

function radioValue(name: string): string {
  return (form.elements.namedItem(name) as RadioNodeList).value
}

function toBigUint64Array(input: string): BigUint64Array {
  const values = input
    .split(',')
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
    .map((part) => BigInt(part))
  return new BigUint64Array(values)
}

function describeReceivedObject(payload: ArrayBuffer | Uint8Array): void {
  const brElement = document.createElement('br')
  receivedTextElement.prepend(brElement)

  const receivedArray = payload instanceof Uint8Array ? payload : new Uint8Array(payload)
  const receivedText = new TextDecoder().decode(receivedArray)

  const receivedElement = document.createElement('p')
  receivedElement.textContent = receivedText
  receivedTextElement.prepend(receivedElement)
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
  field('subscribe-assigned-track-alias').value = aliasValue
  field('unsubscribe-subscribe-id').value = requestId.toString()
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
  field('subscribe-assigned-track-alias').value = nextValue
}

function getSelectedTrackAlias(): bigint {
  const aliasValue = aliasSelect.value
  if (!aliasValue) {
    throw new Error('No active track alias selected')
  }
  return BigInt(aliasValue)
}

function attachSubgroupObjectHandler(trackAlias: bigint): void {
  moqtClient.setOnSubgroupObjectHandler(trackAlias, (receivedGroupId, subgroupObject: SubgroupObjectMessage) => {
    console.info({ groupId: receivedGroupId, subgroupObject })
    if (subgroupObject.objectPayload.length > 0) {
      describeReceivedObject(subgroupObject.objectPayload)
    }
  })
}

function isRequestError(response: SubscribeOkMessage | RequestErrorMessage): response is RequestErrorMessage {
  return 'errorCode' in response
}

function registerClientCallbacks(): void {
  moqtClient.setOnServerSetupHandler((serverSetup) => console.info({ serverSetup }))
  moqtClient.setOnPublishNamespaceHandler(async ({ publishNamespace, respondOk }) => {
    console.info({ publishNamespace })
    await respondOk()
  })
  moqtClient.setOnPublishNamespaceResponseHandler((response) => console.info({ publishNamespaceResponse: response }))
  moqtClient.setOnSubscribeNamespaceResponseHandler((response) =>
    console.info({ subscribeNamespaceResponse: response })
  )
  moqtClient.setOnTrackStatusResponseHandler((response) => {
    if (isRequestError(response)) {
      console.info(
        `TrackStatusError requestId=${response.requestId} code=${response.errorCode} reason=${response.reasonPhrase}`
      )
      return
    }
    console.info(`TrackStatusOk requestId=${response.requestId}`)
  })
  moqtClient.setOnSubscribeResponseHandler((response) => {
    console.info({ subscribeResponse: response })
    if (isRequestError(response)) {
      requestTrackMetadata.delete(response.requestId)
      return
    }

    registerTrackAlias(response.requestId, response.trackAlias, requestTrackMetadata.get(response.requestId))
    attachSubgroupObjectHandler(response.trackAlias)
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
  moqtClient.setOnObjectDatagramHandler((message) => {
    console.info({ objectDatagram: message })
    if (message.objectPayload.length > 0) {
      describeReceivedObject(message.objectPayload)
    }
  })
  moqtClient.setOnObjectDatagramStatusHandler((message) => console.info({ objectDatagramStatus: message }))
  moqtClient.setOnSubgroupHeaderHandler((message) => console.info({ subgroupHeader: message }))
  moqtClient.setOnConnectionClosedHandler(() => {
    console.info('connection closed')
    subgroupHeaderSent.clear()
    requestTrackAliases.clear()
    requestTrackMetadata.clear()
    moqtClient.clearSubgroupObjectHandlers()
    field('subscribe-assigned-track-alias').value = ''
    field('unsubscribe-subscribe-id').value = ''
    aliasSelect.innerHTML = ''
  })
}

function setSendStatus(message: string): void {
  element('send-status').textContent = message
}

async function sendControlMessage(label: string, send: () => Promise<unknown>): Promise<void> {
  try {
    await send()
    setSendStatus(`Sent ${label}`)
  } catch (error) {
    setSendStatus(`Failed ${label}: ${error instanceof Error ? error.message : String(error)}`)
  }
}

function onClick(id: string, handler: () => unknown): void {
  element(id).addEventListener('click', handler)
}

function setupControlMessageButtons(): void {
  onClick('sendPublishNamespaceDoneBtn', async () => {
    const trackNamespace = parseTrackNamespace(field('publish-namespace-done-track-namespace').value)
    await sendControlMessage('PUBLISH_NAMESPACE_DONE', () =>
      requireRawClient(moqtClient).sendPublishNamespaceDone(trackNamespace)
    )
  })

  onClick('sendUnsubscribeNamespaceBtn', async () => {
    const trackNamespacePrefix = parseTrackNamespace(field('unsubscribe-namespace-prefix').value)
    await sendControlMessage('UNSUBSCRIBE_NAMESPACE', () =>
      requireRawClient(moqtClient).sendUnsubscribeNamespace(trackNamespacePrefix)
    )
  })

  onClick('sendPublishBtn', async () => {
    const requestId = BigInt(field('publish-request-id').value)
    const trackNamespace = parseTrackNamespace(field('publish-track-namespace-2').value)
    const trackName = field('publish-track-name').value
    const trackAlias = BigInt(field('publish-track-alias').value)
    const groupOrder = Number(field('publish-group-order').value)
    const authInfo = field('auth-info').value
    await sendControlMessage('PUBLISH', () =>
      requireRawClient(moqtClient).sendPublish(
        requestId,
        trackNamespace,
        trackName,
        trackAlias,
        groupOrder,
        false,
        undefined,
        undefined,
        true,
        authInfo
      )
    )
  })

  onClick('sendFetchBtn', async () => {
    const requestId = BigInt(field('fetch-request-id').value)
    const trackNamespace = parseTrackNamespace(field('fetch-track-namespace').value)
    const trackName = field('fetch-track-name').value
    const startGroup = BigInt(field('fetch-start-group').value)
    const startObject = BigInt(field('fetch-start-object').value)
    const endGroup = BigInt(field('fetch-end-group').value)
    const endObject = BigInt(field('fetch-end-object').value)
    await sendControlMessage('FETCH', () =>
      requireRawClient(moqtClient).sendFetch(
        requestId,
        trackNamespace,
        trackName,
        startGroup,
        startObject,
        endGroup,
        endObject,
        GroupOrder.Ascending
      )
    )
  })

  onClick('sendGoAwayBtn', async () => {
    const newSessionUri = field('goaway-new-session-uri').value
    await sendControlMessage('GOAWAY', () => requireRawClient(moqtClient).sendGoAway(newSessionUri))
  })

  onClick('sendMaxRequestIdBtn', async () => {
    const requestId = BigInt(field('max-request-id-value').value)
    await sendControlMessage('MAX_REQUEST_ID', () => requireRawClient(moqtClient).sendMaxRequestId(requestId))
  })

  onClick('sendRequestsBlockedBtn', async () => {
    const maximumRequestId = BigInt(field('requests-blocked-maximum-request-id').value)
    await sendControlMessage('REQUESTS_BLOCKED', () =>
      requireRawClient(moqtClient).sendRequestsBlocked(maximumRequestId)
    )
  })

  onClick('sendFetchCancelBtn', async () => {
    const requestId = BigInt(field('fetch-cancel-request-id').value)
    await sendControlMessage('FETCH_CANCEL', () => requireRawClient(moqtClient).sendFetchCancel(requestId))
  })

  onClick('sendSubscribeUpdateBtn', async () => {
    const requestId = BigInt(field('subscribe-update-request-id').value)
    const subscriptionRequestId = BigInt(field('subscribe-update-subscription-request-id').value)
    const startGroup = BigInt(field('subscribe-update-start-group').value)
    const startObject = BigInt(field('subscribe-update-start-object').value)
    const endGroup = BigInt(field('subscribe-update-end-group').value)
    const subscriberPriority = Number(field('subscribe-update-subscriber-priority').value)
    const forward = radioValue('forwarding') === 'true'
    await sendControlMessage('SUBSCRIBE_UPDATE', () =>
      requireRawClient(moqtClient).sendSubscribeUpdate(
        requestId,
        subscriptionRequestId,
        startGroup,
        startObject,
        endGroup,
        subscriberPriority,
        forward
      )
    )
  })

  onClick('sendPublishDoneBtn', async () => {
    const requestId = BigInt(field('publish-done-request-id').value)
    const statusCode = BigInt(field('publish-done-status-code').value)
    const streamCount = BigInt(field('publish-done-stream-count').value)
    const errorReason = field('publish-done-error-reason').value
    await sendControlMessage('PUBLISH_DONE', () =>
      requireRawClient(moqtClient).sendPublishDone(requestId, statusCode, streamCount, errorReason)
    )
  })

  onClick('sendPublishNamespaceCancelBtn', async () => {
    const trackNamespace = parseTrackNamespace(field('publish-namespace-cancel-track-namespace').value)
    const errorCode = BigInt(field('publish-namespace-cancel-error-code').value)
    const errorReason = field('publish-namespace-cancel-error-reason').value
    await sendControlMessage('PUBLISH_NAMESPACE_CANCEL', () =>
      requireRawClient(moqtClient).sendPublishNamespaceCancel(trackNamespace, errorCode, errorReason)
    )
  })

  onClick('sendTrackStatusBtn', async () => {
    const requestId = BigInt(field('track-status-request-id').value)
    const trackNamespace = parseTrackNamespace(field('track-status-track-namespace').value)
    const trackName = field('track-status-track-name').value
    const authInfo = field('auth-info').value
    await sendControlMessage('TRACK_STATUS', () =>
      requireRawClient(moqtClient).sendTrackStatus(requestId, trackNamespace, trackName, authInfo)
    )
  })
}

function setupSessionButtons(): void {
  onClick('connectBtn', async () => {
    await moqtClient.connect(field('url').value, { sendSetup: false })
    registerClientCallbacks()
  })

  onClick('closeBtn', async () => {
    await moqtClient.disconnect()
    subgroupHeaderSent.clear()
    requestTrackAliases.clear()
    requestTrackMetadata.clear()
    objectId = 0n
    groupId = 0n
    subgroupId = 0n
  })

  onClick('sendSetupBtn', async () => {
    const versions = toBigUint64Array(field('versions').value)
    const maxRequestId = BigInt(field('max-subscribe-id').value)
    await sendControlMessage('CLIENT_SETUP', () => moqtClient.sendClientSetup(versions, maxRequestId))
  })

  onClick('sendPublishNamespaceBtn', async () => {
    const trackNamespace = parseTrackNamespace(field('publish-track-namespace').value)
    const authInfo = field('auth-info').value
    await sendControlMessage('PUBLISH_NAMESPACE', () => moqtClient.publishNamespace(trackNamespace, authInfo))
  })

  onClick('sendSubscribeNamespaceBtn', async () => {
    const trackNamespacePrefix = parseTrackNamespace(field('track-namespace-prefix').value)
    const authInfo = field('auth-info').value
    await sendControlMessage('SUBSCRIBE_NAMESPACE', () => moqtClient.subscribeNamespace(trackNamespacePrefix, authInfo))
  })

  onClick('sendSubscribeBtn', async () => {
    const requestId = BigInt(field('subscribe-request-id').value)
    const trackNamespace = parseTrackNamespace(field('subscribe-track-namespace').value)
    const trackName = field('track-name').value
    const authInfo = field('auth-info').value
    const subscriberPriority = Number(field('subscriber-priority').value)
    const groupOrder = Number(radioValue('group-order')) as GroupOrder
    const filterType = Number(radioValue('filter-type')) as FilterType
    const startGroup = BigInt(field('start-group').value)
    const startObject = BigInt(field('start-object').value)
    const endGroup = BigInt(field('end-group').value)
    const forward = radioValue('forwarding') === 'true'

    requestTrackMetadata.set(requestId, {
      trackNamespace: [...trackNamespace],
      trackName
    })

    await sendControlMessage('SUBSCRIBE', async () => {
      const trackAlias = (
        await moqtClient.subscribe(trackNamespace, trackName, authInfo, {
          requestId,
          subscriberPriority,
          groupOrder,
          filterType,
          startGroup,
          startObject,
          endGroup,
          forward
        })
      ).subscribeOk.trackAlias

      registerTrackAlias(requestId, trackAlias, {
        trackNamespace: [...trackNamespace],
        trackName
      })
      attachSubgroupObjectHandler(trackAlias)
    })
  })

  onClick('sendUnsubscribeBtn', async () => {
    const requestId = BigInt(field('unsubscribe-subscribe-id').value)
    await sendControlMessage('UNSUBSCRIBE', async () => {
      await moqtClient.unsubscribe(requestId)
      unregisterTrackAliasByRequestId(requestId)
    })
  })
}

function readObjectInputs(): { client: MOQTClient; trackAlias: bigint; publisherPriority: number } {
  return {
    client: requireRawClient(moqtClient),
    trackAlias: getSelectedTrackAlias(),
    publisherPriority: Number(field('publisher-priority').value)
  }
}

function readPayload(): Uint8Array {
  return new TextEncoder().encode(field('payload').value)
}

function readObjectStatus(): number {
  return Number(radioValue('object-status'))
}

function renderIds(): void {
  element('objectId').textContent = objectId.toString()
  element('datagramGroupId').textContent = groupId.toString()
  element('subgroupGroupId').textContent = groupId.toString()
  element('subgroupId').textContent = subgroupId.toString()
}

async function sendSubgroup(objectStatus: number | undefined, payload: Uint8Array): Promise<void> {
  const { client, trackAlias, publisherPriority } = readObjectInputs()
  const key = `${trackAlias.toString()}:${groupId.toString()}:${subgroupId.toString()}`
  if (!subgroupHeaderSent.has(key)) {
    await client.sendSubgroupHeader(trackAlias, groupId, subgroupId, publisherPriority)
    subgroupHeaderSent.add(key)
  }
  await client.sendSubgroupObject(trackAlias, groupId, subgroupId, objectId++, objectStatus, payload, undefined)
  renderIds()
}

function setupObjectButtons(): void {
  onClick('sendDatagramObjectBtn', async () => {
    const { client, trackAlias, publisherPriority } = readObjectInputs()
    await client.sendObjectDatagram(trackAlias, groupId, objectId++, publisherPriority, readPayload(), undefined)
    renderIds()
  })

  onClick('sendDatagramObjectWithStatusBtn', async () => {
    const { client, trackAlias, publisherPriority } = readObjectInputs()
    await client.sendObjectDatagramStatus(
      trackAlias,
      groupId,
      objectId++,
      publisherPriority,
      readObjectStatus(),
      undefined
    )
    renderIds()
  })

  onClick('sendSubgroupObjectBtn', () => sendSubgroup(undefined, readPayload()))
  onClick('sendSubgroupObjectWithStatusBtn', () => sendSubgroup(readObjectStatus(), new Uint8Array()))

  const idButtons: [string, 'group' | 'subgroup', bigint][] = [
    ['ascendDatagramGroupIdBtn', 'group', 1n],
    ['descendDatagramGroupIdBtn', 'group', -1n],
    ['ascendSubgroupGroupIdBtn', 'group', 1n],
    ['descendSubgroupGroupIdBtn', 'group', -1n],
    ['ascendSubgroupIdBtn', 'subgroup', 1n],
    ['descendSubgroupIdBtn', 'subgroup', -1n]
  ]
  for (const [id, counter, step] of idButtons) {
    onClick(id, () => {
      if (counter === 'group') {
        if (groupId + step < 0n) {
          return
        }
        groupId += step
        subgroupId = 0n
      } else {
        if (subgroupId + step < 0n) {
          return
        }
        subgroupId += step
      }
      objectId = 0n
      renderIds()
    })
  }
}

setUpRelayUrlControls(element('url'), element('urlPresets'))
for (const id of ['publish-track-namespace', 'track-namespace-prefix', 'subscribe-track-namespace']) {
  element<HTMLInputElement>(id).value = buildDefaultTrackNamespace()
}
setupSessionButtons()
setupObjectButtons()
setupControlMessageButtons()
