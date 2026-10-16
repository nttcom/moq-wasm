import { expect, test } from '@playwright/test'
import type { MOQTClient, SubgroupObjectMessage } from '../../pkg/moqt'
import { GroupOrder, MoqtClientWrapper } from './moqtClient'

type Callback = (...args: any[]) => unknown

/// A raw client whose relay accepts every request, assigns the track aliases
/// it is given and records what was sent to it.
class FakeRawClient {
  readonly sent: string[] = []
  closed = false
  private connected = false
  private readonly callbacks = new Map<string, Callback>()

  /** SUBSCRIBEs the relay rejects with TRACK_DOES_NOT_EXIST before accepting. */
  rejectSubscribes = 0
  /** The Largest Location the relay reports in SUBSCRIBE_OK, as [group, object]. */
  largest?: [bigint, bigint]
  readonly subscribeFilters: number[] = []

  constructor(
    readonly url: string,
    private readonly nextAliases: bigint[]
  ) {}

  private on(name: string) {
    return (callback: Callback) => {
      this.callbacks.set(name, callback)
    }
  }

  readonly onServerSetup = this.on('serverSetup')
  readonly onPublishNamespace = this.on('publishNamespace')
  readonly onPublishNamespaceDone = this.on('publishNamespaceDone')
  readonly onPublishNamespaceResponse = this.on('publishNamespaceResponse')
  readonly onSubscribeNamespaceResponse = this.on('subscribeNamespaceResponse')
  readonly onPublishResponse = this.on('publishResponse')
  readonly onSubscribe = this.on('subscribe')
  readonly onSubscribeResponse = this.on('subscribeResponse')
  readonly onPublishDone = this.on('publishDone')
  readonly onIncomingUnsubscribe = this.on('incomingUnsubscribe')
  readonly onObjectDatagram = this.on('objectDatagram')
  readonly onObjectDatagramStatus = this.on('objectDatagramStatus')
  readonly onSubgroupHeader = this.on('subgroupHeader')
  readonly onSubgroupObject = this.on('subgroupObject')
  readonly onSubgroupEnd = this.on('subgroupEnd')
  readonly onTrackStatus = this.on('trackStatus')
  readonly onTrackStatusResponse = this.on('trackStatusResponse')
  readonly onFetch = this.on('fetch')
  readonly onFetchCancel = this.on('fetchCancel')
  readonly onFetchResponse = this.on('fetchResponse')
  readonly onFetchObject = this.on('fetchObject')
  readonly onFetchStreamEnd = this.on('fetchStreamEnd')
  readonly onGoAway = this.on('goAway')
  readonly onConnectionClosed = this.on('connectionClosed')

  private emit(name: string, ...args: unknown[]): void {
    queueMicrotask(() => this.callbacks.get(name)?.(...args))
  }

  isConnected(): boolean {
    return this.connected
  }

  async start(): Promise<void> {
    this.connected = true
  }

  async close(): Promise<void> {
    this.closed = true
    this.connected = false
  }

  async sendClientSetup(): Promise<void> {
    this.sent.push('CLIENT_SETUP')
    this.emit('serverSetup', { selectedVersion: 0xff00000en })
  }

  async sendPublishNamespace(requestId: bigint, trackNamespace: string[]): Promise<void> {
    this.sent.push(`PUBLISH_NAMESPACE ${trackNamespace.join('/')}`)
    this.emit('publishNamespaceResponse', { requestId })
  }

  async sendSubscribeNamespace(requestId: bigint, prefix: string[]): Promise<void> {
    this.sent.push(`SUBSCRIBE_NAMESPACE ${prefix.join('/')}`)
    this.emit('subscribeNamespaceResponse', { requestId })
  }

  async sendPublishNamespaceOk(requestId: bigint): Promise<void> {
    this.sent.push(`PUBLISH_NAMESPACE_OK ${requestId}`)
  }

  async sendSubscribe(
    requestId: bigint,
    trackNamespace: string[],
    trackName: string,
    _subscriberPriority: number,
    _groupOrder: number,
    filterType: number
  ): Promise<void> {
    this.sent.push(`SUBSCRIBE ${requestId} ${trackNamespace.join('/')}/${trackName}`)
    this.subscribeFilters.push(filterType)
    if (this.rejectSubscribes > 0) {
      this.rejectSubscribes -= 1
      this.emit('subscribeResponse', { requestId, errorCode: 4n, reasonPhrase: 'track does not exist yet' })
      return
    }
    const trackAlias = this.nextAliases.shift()
    if (trackAlias === undefined) {
      throw new Error('fake relay ran out of aliases')
    }
    this.emit('subscribeResponse', {
      requestId,
      trackAlias,
      contentExists: this.largest !== undefined,
      largestGroupId: this.largest?.[0],
      largestObjectId: this.largest?.[1]
    })
  }

  async sendUnsubscribe(requestId: bigint): Promise<void> {
    this.sent.push(`UNSUBSCRIBE ${requestId}`)
  }

  async sendSubscribeForward(_requestId: bigint, subscriptionRequestId: bigint, forward: boolean): Promise<void> {
    this.sent.push(`SUBSCRIBE_UPDATE ${subscriptionRequestId} forward=${forward}`)
  }

  async sendPublish(requestId: bigint, trackNamespace: string[], trackName: string): Promise<bigint> {
    this.sent.push(`PUBLISH ${requestId} ${trackNamespace.join('/')}/${trackName}`)
    const trackAlias = this.nextAliases.shift()
    if (trackAlias === undefined) {
      throw new Error('fake relay ran out of aliases')
    }
    this.emit('publishResponse', { requestId })
    return trackAlias
  }

  async sendSubscribeOk(requestId: bigint): Promise<bigint> {
    this.sent.push(`SUBSCRIBE_OK ${requestId}`)
    const trackAlias = this.nextAliases.shift()
    if (trackAlias === undefined) {
      throw new Error('fake relay ran out of aliases')
    }
    return trackAlias
  }

  async sendSubgroupHeader(trackAlias: bigint, groupId: bigint): Promise<void> {
    this.sent.push(`SUBGROUP_HEADER alias=${trackAlias} group=${groupId}`)
  }

  async sendSubgroupObject(trackAlias: bigint, groupId: bigint, _subgroupId: bigint, objectId: bigint): Promise<void> {
    this.sent.push(`OBJECT alias=${trackAlias} group=${groupId} object=${objectId}`)
  }

  async sendFetch(
    requestId: bigint,
    trackNamespace: string[],
    trackName: string,
    startGroupId: bigint,
    startObjectId: bigint,
    endGroupId: bigint,
    endObjectId: bigint
  ): Promise<void> {
    this.sent.push(
      `FETCH ${requestId} ${trackNamespace.join('/')}/${trackName} ${startGroupId}:${startObjectId}-${endGroupId}:${endObjectId}`
    )
    this.emit('fetchResponse', { requestId })
  }

  async sendRelativeJoiningFetch(requestId: bigint, joiningRequestId: bigint, joiningStart: bigint): Promise<void> {
    this.sent.push(`JOINING_FETCH ${requestId} ${joiningRequestId} ${joiningStart}`)
    this.emit('fetchResponse', { requestId })
  }

  receiveFetchObject(requestId: bigint, groupId: bigint, objectId: bigint): void {
    this.emit('fetchObject', { requestId, groupId, objectId, objectPayload: new Uint8Array(0) })
  }

  receiveFetchEnd(requestId: bigint): void {
    this.emit('fetchStreamEnd', { requestId, isReset: false })
  }

  receivePublishNamespace(requestId: bigint, trackNamespace: string[]): void {
    this.emit('publishNamespace', { requestId, trackNamespace })
  }

  receiveSubscribe(requestId: bigint, trackNamespace: string[], trackName: string): void {
    this.emit('subscribe', { requestId, trackNamespace, trackName }, true, 0)
  }

  receiveUnsubscribe(requestId: bigint): void {
    this.emit('incomingUnsubscribe', requestId)
  }

  receivePublishDone(requestId: bigint): void {
    this.emit('publishDone', { requestId, statusCode: 0x2n, errorReason: 'publisher moved away' })
  }

  receiveGoAway(newSessionUri: string): void {
    this.emit('goAway', newSessionUri)
  }

  receiveSubgroupHeader(wireAlias: bigint, groupId: bigint): void {
    this.emit('subgroupHeader', { trackAlias: wireAlias, groupId, subgroupId: 0n, publisherPriority: 0 })
  }

  receiveSubgroupEnd(wireAlias: bigint, groupId: bigint): void {
    this.emit('subgroupEnd', wireAlias, groupId, 0n)
  }

  receiveSubgroupObject(wireAlias: bigint, groupId: bigint, objectId: bigint): void {
    this.emit('subgroupObject', wireAlias, groupId, { objectId } as unknown as SubgroupObjectMessage)
  }
}

async function settled(): Promise<void> {
  for (let i = 0; i < 20; i++) {
    await new Promise<void>((resolve) => setTimeout(resolve, 0))
  }
}

function fakeRelays(options: { relayBRejectsSubscribes?: number; relayBLargest?: [bigint, bigint] } = {}): {
  clients: FakeRawClient[]
  wrapper: MoqtClientWrapper
} {
  const clients: FakeRawClient[] = []
  const wrapper = new MoqtClientWrapper(async (url) => {
    const client = new FakeRawClient(url, url.includes('relay-b') ? [40n, 41n] : [7n, 8n])
    if (url.includes('relay-b')) {
      client.rejectSubscribes = options.relayBRejectsSubscribes ?? 0
      client.largest = options.relayBLargest
    }
    clients.push(client)
    return client as unknown as MOQTClient
  })
  return { clients, wrapper }
}

test('GOAWAY moves the namespaces and subscriptions to the named relay under the same ids and aliases', async () => {
  // Arrange: announced, subscribed to a prefix and to one track (alias 7 on relay A), group 4 arriving
  const { clients, wrapper } = fakeRelays()
  await wrapper.connect('https://relay-a.example:4433')
  await wrapper.publishNamespace(['room', 'me'], '')
  await wrapper.subscribeNamespace(['room'], '')
  const { requestId, subscribeOk } = await wrapper.subscribe(['room', 'them'], 'video', '', { forward: true })
  const received: bigint[] = []
  wrapper.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (groupId) => received.push(groupId))
  const [relayA] = clients
  relayA.receiveSubgroupHeader(7n, 4n)
  await settled()

  // Act
  relayA.receiveGoAway('https://relay-b.example:4433')
  await settled()

  // Assert: relay B holds the same state under the same request id
  expect(clients).toHaveLength(2)
  const relayB = clients[1]
  expect(relayB.url).toBe('https://relay-b.example:4433')
  expect(relayB.sent).toEqual([
    'CLIENT_SETUP',
    'PUBLISH_NAMESPACE room/me',
    'SUBSCRIBE_NAMESPACE room',
    `SUBSCRIBE ${requestId} room/them/video`
  ])
  expect(wrapper.getRawClient()).toBe(relayB as unknown as MOQTClient)

  // Assert: objects arriving under relay B's alias 40 reach the handler registered for alias 7,
  // and relay A is closed once it, too, reaches that group
  relayB.receiveSubgroupObject(40n, 5n, 0n)
  await settled()
  expect(relayA.closed).toBe(false)
  relayA.receiveSubgroupObject(7n, 5n, 0n)
  await settled()
  expect(received).toEqual([5n])
  expect(relayA.closed).toBe(true)

  // Assert: the application's request id still names the subscription on relay B
  await wrapper.unsubscribe(requestId)
  expect(relayB.sent).toContain(`UNSUBSCRIBE ${requestId}`)
})

test('GOAWAY with an empty URI reconnects to the current URL', async () => {
  // Arrange
  const { clients, wrapper } = fakeRelays()
  await wrapper.connect('https://relay-a.example:4433')
  const { requestId } = await wrapper.subscribe(['room', 'them'], 'audio', '')

  // Act
  clients[0].receiveGoAway('')
  await settled()
  clients[0].receivePublishDone(requestId)
  await settled()

  // Assert: the old session closes once its relay ended the subscription
  expect(clients).toHaveLength(2)
  expect(clients[1].url).toBe('https://relay-a.example:4433')
  expect(clients[0].closed).toBe(true)
})

test('a paused subscription stays paused on the new relay', async () => {
  // Arrange
  const { clients, wrapper } = fakeRelays()
  await wrapper.connect('https://relay-a.example:4433')
  const { requestId } = await wrapper.subscribe(['room', 'them'], 'video', '', { forward: true })
  await wrapper.setSubscriptionForward(requestId, false)

  // Act
  clients[0].receiveGoAway('https://relay-b.example:4433')
  await settled()

  // Assert
  expect(clients[1].sent).toContain(`SUBSCRIBE_UPDATE ${requestId} forward=false`)
})

test('a failed switchover keeps the current session', async () => {
  // Arrange
  const clients: FakeRawClient[] = []
  const wrapper = new MoqtClientWrapper(async (url) => {
    if (url.includes('relay-b')) {
      throw new Error('connection refused')
    }
    const client = new FakeRawClient(url, [7n])
    clients.push(client)
    return client as unknown as MOQTClient
  })
  await wrapper.connect('https://relay-a.example:4433')
  await wrapper.subscribe(['room', 'them'], 'video', '')

  // Act
  clients[0].receiveGoAway('https://relay-b.example:4433')
  await settled()

  // Assert
  expect(clients[0].closed).toBe(false)
  expect(wrapper.getRawClient()).toBe(clients[0] as unknown as MOQTClient)
})

test('a published track keeps its alias for the application while the new relay gets its own', async () => {
  // Arrange
  const { clients, wrapper } = fakeRelays()
  await wrapper.connect('https://relay-a.example:4433')
  const appAlias = await wrapper.publish(['room', 'me'], 'video', '')
  expect(appAlias).toBe(7n)

  // Act
  clients[0].receiveGoAway('https://relay-b.example:4433')
  await settled()
  await wrapper.sendSubgroupHeader(appAlias, 3n, 0n, 0)
  await wrapper.sendSubgroupObject(appAlias, 3n, 0n, 0n, undefined, new Uint8Array(1), undefined)

  // Assert: relay B received PUBLISH and the objects under its own alias 40
  const relayB = clients[1]
  expect(relayB.sent.some((line) => line.startsWith('PUBLISH ') && line.endsWith(' room/me/video'))).toBe(true)
  expect(relayB.sent).toContain('SUBGROUP_HEADER alias=40 group=3')
  expect(relayB.sent).toContain('OBJECT alias=40 group=3 object=0')
})

test('a subscribed track goes to both relays until the old relay unsubscribes', async () => {
  // Arrange: relay A subscribed to a track this client serves (alias 7) before the switch, relay B after it (alias 40)
  const { clients, wrapper } = fakeRelays()
  await wrapper.connect('https://relay-a.example:4433')
  wrapper.setOnIncomingSubscribeHandler(async ({ respondOk }) => {
    await respondOk()
  })
  const ended: bigint[] = []
  wrapper.setOnIncomingUnsubscribeHandler((requestId) => ended.push(requestId))
  const [relayA] = clients
  relayA.receiveSubscribe(11n, ['room', 'me'], 'video')
  await settled()
  relayA.receiveGoAway('https://relay-b.example:4433')
  await settled()
  const relayB = clients[1]
  relayB.receiveSubscribe(13n, ['room', 'me'], 'video')
  await settled()

  // Act
  const aliases = wrapper.trackSubscribers(['room', 'me'], 'video')
  for (const alias of aliases) {
    await wrapper.sendSubgroupHeader(alias, 5n, 0n, 0)
    await wrapper.sendSubgroupObject(alias, 5n, 0n, 0n, undefined, new Uint8Array(1), undefined)
  }
  const closedWhileSubscribed = relayA.closed
  relayA.receiveUnsubscribe(11n)
  await settled()

  // Assert
  expect(aliases).toEqual([7n, 40n])
  expect(relayA.sent).toEqual(
    expect.arrayContaining(['SUBGROUP_HEADER alias=7 group=5', 'OBJECT alias=7 group=5 object=0'])
  )
  expect(relayB.sent).toEqual(
    expect.arrayContaining(['SUBGROUP_HEADER alias=40 group=5', 'OBJECT alias=40 group=5 object=0'])
  )
  expect(closedWhileSubscribed).toBe(false)
  expect(ended).toEqual([11n])
  expect(wrapper.trackSubscribers(['room', 'me'], 'video')).toEqual([40n])
  expect(relayA.closed).toBe(true)
})

test('an alias the new relay gives a subscriber that the application already sends on is replaced', async () => {
  // Arrange: a published track holds alias 7; relay B maps it to 40 and gives 7 to a new subscriber
  const clients: FakeRawClient[] = []
  const wrapper = new MoqtClientWrapper(async (url) => {
    const client = new FakeRawClient(url, url.includes('relay-b') ? [40n, 7n] : [7n])
    clients.push(client)
    return client as unknown as MOQTClient
  })
  await wrapper.connect('https://relay-a.example:4433')
  wrapper.setOnIncomingSubscribeHandler(async ({ respondOk }) => {
    await respondOk()
  })
  const published = await wrapper.publish(['room', 'me'], 'video', '')
  clients[0].receiveGoAway('https://relay-b.example:4433')
  await settled()
  const relayB = clients[1]
  relayB.receiveSubscribe(13n, ['room', 'me'], 'audio')
  await settled()

  // Act
  const [subscribed] = wrapper.trackSubscribers(['room', 'me'], 'audio')
  await wrapper.sendSubgroupHeader(subscribed, 2n, 0n, 0)
  await wrapper.sendSubgroupHeader(published, 2n, 0n, 0)

  // Assert
  expect(published).toBe(7n)
  expect(subscribed).not.toBe(published)
  expect(relayB.sent).toEqual(
    expect.arrayContaining(['SUBGROUP_HEADER alias=7 group=2', 'SUBGROUP_HEADER alias=40 group=2'])
  )
})

test('an in-flight FETCH resumes on the new relay after the last object delivered', async () => {
  // Arrange: groups 1 to 5 requested, objects up to 2:1 delivered by relay A
  const { clients, wrapper } = fakeRelays()
  await wrapper.connect('https://relay-a.example:4433')
  const delivered: string[] = []
  let ended = 0
  const { requestId } = await wrapper.fetch(['room', 'them'], 'video', 1n, 0n, 5n, 0n, {
    onObject: (message) => delivered.push(`${message.groupId}:${message.objectId}`),
    onStreamEnd: () => ended++
  })
  const [relayA] = clients
  relayA.receiveFetchObject(requestId, 1n, 0n)
  relayA.receiveFetchObject(requestId, 2n, 0n)
  relayA.receiveFetchObject(requestId, 2n, 1n)
  await settled()

  // Act
  relayA.receiveGoAway('https://relay-b.example:4433')
  await settled()
  const relayB = clients[1]
  relayA.receiveFetchObject(requestId, 2n, 2n)
  relayB.receiveFetchObject(requestId, 3n, 0n)
  relayB.receiveFetchEnd(requestId)
  await settled()

  // Assert: relay B was asked for the rest, relay A's late object was dropped, the stream ended once
  expect(relayB.sent).toContain(`FETCH ${requestId} room/them/video 2:2-5:0`)
  expect(delivered).toEqual(['1:0', '2:0', '2:1', '3:0'])
  expect(ended).toBe(1)
})

test('a joining FETCH that delivered nothing yet is repeated against the re-issued subscription', async () => {
  // Arrange
  const { clients, wrapper } = fakeRelays()
  await wrapper.connect('https://relay-a.example:4433')
  const { requestId: subscribeId } = await wrapper.subscribe(['room', 'them'], 'catalog', '')
  const { requestId } = await wrapper.relativeJoiningFetch(subscribeId, 0n)

  // Act
  clients[0].receiveGoAway('https://relay-b.example:4433')
  await settled()

  // Assert
  expect(clients[1].sent).toContain(`JOINING_FETCH ${requestId} ${subscribeId} 0`)
})

test('a descending FETCH resumes on the new relay without repeating the objects already delivered', async () => {
  // Arrange: groups 5 down to 1 requested, relay A delivered 4:0, 4:1 and 3:0
  const { clients, wrapper } = fakeRelays()
  await wrapper.connect('https://relay-a.example:4433')
  const delivered: string[] = []
  const { requestId } = await wrapper.fetch(['room', 'them'], 'video', 1n, 0n, 5n, 0n, {
    groupOrder: GroupOrder.Descending,
    onObject: (message) => delivered.push(`${message.groupId}:${message.objectId}`)
  })
  const [relayA] = clients
  relayA.receiveFetchObject(requestId, 4n, 0n)
  relayA.receiveFetchObject(requestId, 4n, 1n)
  relayA.receiveFetchObject(requestId, 3n, 0n)
  await settled()

  // Act
  relayA.receiveGoAway('https://relay-b.example:4433')
  await settled()
  const relayB = clients[1]
  relayB.receiveFetchObject(requestId, 3n, 0n)
  relayB.receiveFetchObject(requestId, 3n, 1n)
  relayB.receiveFetchObject(requestId, 2n, 0n)
  await settled()

  // Assert: the range was cut after group 3 and group 3's repeated object was dropped
  expect(relayB.sent).toContain(`FETCH ${requestId} room/them/video 1:0-4:0`)
  expect(delivered).toEqual(['4:0', '4:1', '3:0', '3:1', '2:0'])
})

test('a SUBSCRIBE the new relay rejects is repeated until the publisher has arrived there', async () => {
  // Arrange: relay B knows the track only after the publisher moved as well
  const { clients, wrapper } = fakeRelays({ relayBRejectsSubscribes: 1 })
  await wrapper.connect('https://relay-a.example:4433')
  const { requestId } = await wrapper.subscribe(['room', 'them'], 'video', '')

  // Act
  clients[0].receiveGoAway('https://relay-b.example:4433')
  await new Promise<void>((resolve) => setTimeout(resolve, 700))
  await settled()

  // Assert
  const subscribes = clients[1].sent.filter((line) => line === `SUBSCRIBE ${requestId} room/them/video`)
  expect(subscribes).toHaveLength(2)
  expect(wrapper.getRawClient()).toBe(clients[1] as unknown as MOQTClient)
})

test('a PUBLISH_DONE from the relay that sent GOAWAY does not end the subscription carried over', async () => {
  // Arrange: the publisher has already moved, so relay A ends the subscription
  const { clients, wrapper } = fakeRelays()
  await wrapper.connect('https://relay-a.example:4433')
  const ended: bigint[] = []
  const { requestId } = await wrapper.subscribe(['room', 'them'], 'video', '', {
    onPublishDone: (message) => ended.push(message.requestId)
  })

  // Act
  clients[0].receiveGoAway('https://relay-b.example:4433')
  clients[0].receivePublishDone(requestId)
  await settled()

  // Assert
  expect(ended).toEqual([])
  expect(clients[1].sent).toContain(`SUBSCRIBE ${requestId} room/them/video`)
})

/// Relay B assigns the aliases the other way round from relay A and accepts
/// the second SUBSCRIBE only on its retry, so the switch is still under way.
function relaysWithSwappedAliases(): { clients: FakeRawClient[]; wrapper: MoqtClientWrapper } {
  const clients: FakeRawClient[] = []
  const wrapper = new MoqtClientWrapper(async (url) => {
    const client = new FakeRawClient(url, url.includes('relay-b') ? [8n, 7n] : [7n, 8n])
    clients.push(client)
    return client as unknown as MOQTClient
  })
  return { clients, wrapper }
}

test('an object the new relay sends before the switch completes reaches its own track', async () => {
  // Arrange: video is alias 7 and audio alias 8 on relay A; relay B uses 8 for video
  const { clients, wrapper } = relaysWithSwappedAliases()
  await wrapper.connect('https://relay-a.example:4433')
  const video = await wrapper.subscribe(['room', 'them'], 'video', '')
  const audio = await wrapper.subscribe(['room', 'them'], 'audio', '')
  const videoGroups: bigint[] = []
  const audioGroups: bigint[] = []
  wrapper.setOnSubgroupObjectHandler(video.subscribeOk.trackAlias, (groupId) => videoGroups.push(groupId))
  wrapper.setOnSubgroupObjectHandler(audio.subscribeOk.trackAlias, (groupId) => audioGroups.push(groupId))

  // Act: relay B accepts video and sends on it while audio is still pending there
  clients[0].receiveGoAway('https://relay-b.example:4433')
  await settled()
  const relayB = clients[1]
  relayB.rejectSubscribes = 0
  relayB.receiveSubgroupObject(8n, 3n, 0n)
  await settled()

  // Assert
  expect(videoGroups).toEqual([3n])
  expect(audioGroups).toEqual([])
})

test('a group the old relay started is taken from it, and later groups only from the new relay', async () => {
  // Arrange: relay A delivered the first objects of group 4
  const { clients, wrapper } = fakeRelays()
  await wrapper.connect('https://relay-a.example:4433')
  const { subscribeOk } = await wrapper.subscribe(['room', 'them'], 'video', '')
  const received: string[] = []
  wrapper.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (groupId, object) =>
    received.push(`${groupId}:${object.objectId}`)
  )
  const [relayA] = clients
  relayA.receiveSubgroupHeader(7n, 4n)
  relayA.receiveSubgroupObject(7n, 4n, 0n)
  relayA.receiveSubgroupObject(7n, 4n, 1n)
  await settled()

  // Act: both relays send group 4 and group 5
  relayA.receiveGoAway('https://relay-b.example:4433')
  await settled()
  const relayB = clients[1]
  relayB.receiveSubgroupObject(40n, 4n, 0n)
  relayB.receiveSubgroupObject(40n, 5n, 0n)
  relayA.receiveSubgroupObject(7n, 4n, 2n)
  relayA.receiveSubgroupObject(7n, 5n, 0n)
  await settled()

  // Assert: group 4 whole from relay A, group 5 once from relay B, then relay A closed
  expect(received).toEqual(['4:0', '4:1', '5:0', '4:2'])
  expect(relayA.closed).toBe(true)
})

test('a published track goes to both relays until the old one unsubscribes', async () => {
  // Arrange: group 3 is open on relay A when the switch completes
  const { clients, wrapper } = fakeRelays()
  await wrapper.connect('https://relay-a.example:4433')
  const appAlias = await wrapper.publish(['room', 'me'], 'video', '')
  await wrapper.sendSubgroupHeader(appAlias, 3n, 0n, 0)
  const [relayA] = clients
  const publishRequestId = BigInt(relayA.sent.find((line) => line.startsWith('PUBLISH '))?.split(' ')[1] ?? '-1')
  relayA.receiveGoAway('https://relay-b.example:4433')
  await settled()
  const relayB = clients[1]

  // Act: the rest of group 3 and all of group 4, then relay A unsubscribes
  await wrapper.sendSubgroupObject(appAlias, 3n, 0n, 1n, undefined, new Uint8Array(1), undefined)
  await wrapper.sendSubgroupHeader(appAlias, 4n, 0n, 0)
  await wrapper.sendSubgroupObject(appAlias, 4n, 0n, 0n, undefined, new Uint8Array(1), undefined)
  const closedWhileSubscribed = relayA.closed
  relayA.receiveUnsubscribe(publishRequestId)
  await settled()

  // Assert: relay A got every object, relay B the groups that started after the switch
  expect(relayA.sent).toEqual(
    expect.arrayContaining([
      'OBJECT alias=7 group=3 object=1',
      'SUBGROUP_HEADER alias=7 group=4',
      'OBJECT alias=7 group=4 object=0'
    ])
  )
  expect(relayB.sent).toEqual(
    expect.arrayContaining(['SUBGROUP_HEADER alias=40 group=4', 'OBJECT alias=40 group=4 object=0'])
  )
  expect(relayB.sent).not.toContain('OBJECT alias=40 group=3 object=1')
  expect(closedWhileSubscribed).toBe(false)
  expect(relayA.closed).toBe(true)
})

test('a group the new relay started before the subscription moved is fetched from its start', async () => {
  // Arrange: relay A delivered group 4; relay B already has group 5 up to object 1
  const { clients, wrapper } = fakeRelays({ relayBLargest: [5n, 1n] })
  await wrapper.connect('https://relay-a.example:4433')
  const { requestId, subscribeOk } = await wrapper.subscribe(['room', 'them'], 'video', '', { forward: true })
  const received: string[] = []
  wrapper.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (groupId, object) =>
    received.push(`${groupId}:${object.objectId}`)
  )
  const [relayA] = clients
  relayA.receiveSubgroupObject(7n, 4n, 0n)
  await settled()

  // Act: the subscription moves; relay B's objects after its Largest Location race the FETCH
  relayA.receiveGoAway('https://relay-b.example:4433')
  await settled()
  const relayB = clients[1]
  const fetchLine = relayB.sent.find((line) => line.startsWith('JOINING_FETCH '))
  const fetchRequestId = BigInt(fetchLine?.split(' ')[1] ?? '-1')
  relayB.receiveSubgroupObject(40n, 5n, 2n)
  relayB.receiveFetchObject(fetchRequestId, 5n, 0n)
  relayB.receiveFetchObject(fetchRequestId, 5n, 1n)
  relayB.receiveFetchEnd(fetchRequestId)
  await settled()

  // Assert: relay B was asked from its Largest Location on, and group 5 arrives in order
  expect(relayB.subscribeFilters).toEqual([0x2])
  expect(fetchLine).toBe(`JOINING_FETCH ${fetchRequestId} ${requestId} 0`)
  expect(received).toEqual(['4:0', '5:0', '5:1', '5:2'])
})

test('a subscription with nothing arriving on the old relay leaves it at once', async () => {
  // Arrange
  const { clients, wrapper } = fakeRelays()
  await wrapper.connect('https://relay-a.example:4433')
  await wrapper.subscribe(['room', 'them'], 'chat', '')
  const [relayA] = clients
  relayA.receiveSubgroupHeader(7n, 1n)
  relayA.receiveSubgroupObject(7n, 1n, 0n)
  relayA.receiveSubgroupEnd(7n, 1n)
  await settled()

  // Act
  relayA.receiveGoAway('https://relay-b.example:4433')
  await settled()

  // Assert
  expect(relayA.closed).toBe(true)
})

test('the old relay is left once its open subgroup ends', async () => {
  // Arrange
  const { clients, wrapper } = fakeRelays()
  await wrapper.connect('https://relay-a.example:4433')
  await wrapper.subscribe(['room', 'them'], 'video', '')
  const [relayA] = clients
  relayA.receiveSubgroupHeader(7n, 3n)
  await settled()
  relayA.receiveGoAway('https://relay-b.example:4433')
  await settled()
  const openAfterSwitch = !relayA.closed

  // Act
  relayA.receiveSubgroupEnd(7n, 3n)
  await settled()

  // Assert
  expect(openAfterSwitch).toBe(true)
  expect(relayA.closed).toBe(true)
})

test('a PUBLISH_NAMESPACE the new relay sends before the switch completes is answered there', async () => {
  // Arrange: relay B accepts the carried SUBSCRIBE only on its retry, so the switch is still under way
  const { clients, wrapper } = fakeRelays({ relayBRejectsSubscribes: 1 })
  await wrapper.connect('https://relay-a.example:4433')
  await wrapper.subscribeNamespace(['room'], '')
  await wrapper.subscribe(['room', 'them'], 'video', '')
  const [relayA] = clients
  relayA.receiveGoAway('https://relay-b.example:4433')
  await settled()
  const relayB = clients[1]
  const switchedBefore = wrapper.getRawClient() === (relayB as unknown as MOQTClient)

  // Act
  relayB.receivePublishNamespace(3n, ['room', 'them'])
  await settled()

  // Assert
  expect(switchedBefore).toBe(false)
  expect(relayB.sent).toContain('PUBLISH_NAMESPACE_OK 3')
  expect(relayA.sent).not.toContain('PUBLISH_NAMESPACE_OK 3')
})
