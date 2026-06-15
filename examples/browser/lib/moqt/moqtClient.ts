import init, {
  FetchObjectMessage,
  FetchOkMessage,
  MOQTClient,
  NamespaceOkMessage,
  ObjectDatagramMessage,
  ObjectDatagramStatusMessage,
  PublishNamespaceDoneMessage,
  PublishNamespaceMessage,
  RequestErrorMessage,
  ServerSetupMessage,
  SubgroupHeaderMessage,
  SubgroupObjectMessage,
  SubgroupState,
  SubscribeMessage,
  SubscribeOkMessage
} from '../../pkg/moqt_client_wasm'
import {
  InMemorySubscriptionStateManager,
  SubscriptionStateStore,
  SubgroupObjectHandler
} from './subscriptionStateManager'

type SetupResolver = ((value: void) => void) | null
type PendingVoidResolver = { resolve: () => void; reject: (error: Error) => void }
type PendingSubscribeResolver = { resolve: (response: SubscribeOkMessage) => void; reject: (error: Error) => void }
type FetchResponseHandler = ((response: FetchOkMessage | RequestErrorMessage) => void) | null
type FetchObjectHandler = ((message: FetchObjectMessage) => void) | null
type SubscribeResponseHandler = ((response: SubscribeOkMessage | RequestErrorMessage) => void) | null
type NamespaceResponseHandler = ((response: NamespaceOkMessage | RequestErrorMessage) => void) | null
type ConnectionClosedHandler = (() => void) | null
type IncomingUnsubscribeHandler = ((requestId: bigint) => void) | null
type ObjectDatagramHandler = ((message: ObjectDatagramMessage) => void) | null
type ObjectDatagramStatusHandler = ((message: ObjectDatagramStatusMessage) => void) | null
type SubgroupHeaderHandler = ((header: SubgroupHeaderMessage) => void) | null

export interface ConnectOptions {
  sendSetup?: boolean
  versions?: BigUint64Array
  maxRequestId?: bigint
}

export interface PublishNamespaceOptions {
  requestId?: bigint
}

export interface SubscribeNamespaceOptions {
  requestId?: bigint
}

export interface SubscribeOptions {
  /**
   * Override the request id. When omitted, a session-unique id is issued
   * internally via issueRequestId(). Production code should let it be issued
   * automatically; only manual/test tools that need to control the exact id on
   * the wire should pass this.
   */
  requestId?: bigint
  subscriberPriority?: number
  groupOrder?: number
  filterType?: number
  startGroup?: bigint
  startObject?: bigint
  endGroup?: bigint
  forward?: boolean
  deliveryTimeout?: bigint
}

export interface FetchOptions {
  /** Override the request id; issued internally when omitted. */
  requestId?: bigint
  /** Handler for FETCH objects, registered before the request is sent. */
  onObject?: FetchObjectHandler
}

/** Result of subscribe(): the id we issued plus the SUBSCRIBE_OK response. */
export interface SubscribeResult {
  requestId: bigint
  subscribeOk: SubscribeOkMessage
}

/** Result of fetch()/relativeJoiningFetch(): the id we issued plus FETCH_OK. */
export interface FetchResult {
  requestId: bigint
  fetchOk: FetchOkMessage
}

export interface IncomingPublishNamespaceContext {
  publishNamespace: PublishNamespaceMessage
  respondOk(): Promise<void>
  respondError(code: bigint, reasonPhrase: string): Promise<void>
}

export interface IncomingSubscribeContext {
  subscribe: SubscribeMessage
  isSuccess: boolean
  code: number
  respondOk(
    expires?: bigint,
    contentExists?: boolean,
    largestGroupId?: bigint,
    largestObjectId?: bigint,
    deliveryTimeout?: bigint,
    maxDuration?: bigint
  ): Promise<bigint>
  respondError(code: bigint, reasonPhrase: string): Promise<void>
}

type IncomingPublishNamespaceHandler = (ctx: IncomingPublishNamespaceContext) => Promise<void> | void
type IncomingPublishNamespaceDoneHandler = (message: PublishNamespaceDoneMessage) => void
type IncomingSubscribeHandler = (ctx: IncomingSubscribeContext) => Promise<void> | void

export class MoqtClientWrapper {
  client: MOQTClient | null = null
  private serverSetupResolve: SetupResolver = null
  private onPublishNamespaceHandler: IncomingPublishNamespaceHandler | null = null
  private onPublishNamespaceDoneHandler: IncomingPublishNamespaceDoneHandler | null = null
  private onPublishNamespaceResponseHandler: NamespaceResponseHandler = null
  private onSubscribeNamespaceResponseHandler: NamespaceResponseHandler = null
  private onSubscribeResponseHandler: SubscribeResponseHandler = null
  private onConnectionClosedHandler: ConnectionClosedHandler = null
  private incomingSubscribeHandler: IncomingSubscribeHandler | null = null
  private incomingUnsubscribeHandler: IncomingUnsubscribeHandler = null
  private onServerSetupHandler: ((setup: ServerSetupMessage) => void) | null = null
  private onObjectDatagramHandler: ObjectDatagramHandler = null
  private onObjectDatagramStatusHandler: ObjectDatagramStatusHandler = null
  private onSubgroupHeaderHandler: SubgroupHeaderHandler = null
  private onFetchResponseHandler: FetchResponseHandler = null
  private readonly fetchObjectHandlers = new Map<bigint, FetchObjectHandler>()
  private readonly subscriptionState: SubscriptionStateStore
  private readonly pendingPublishNamespace = new Map<bigint, PendingVoidResolver>()
  private readonly pendingSubscribeNamespace = new Map<bigint, PendingVoidResolver>()
  private readonly pendingSubscribe = new Map<bigint, PendingSubscribeResolver>()
  private readonly subscriptionTrackAliases = new Map<bigint, bigint>()
  private nextRequestId = 0n

  constructor(subscriptionState?: SubscriptionStateStore) {
    this.subscriptionState = subscriptionState ?? new InMemorySubscriptionStateManager()
  }

  async connect(url: string, options: ConnectOptions = {}): Promise<void> {
    if (this.getConnectionStatus()) {
      return
    }

    let wtConnectStartedAtMs: number | null = null
    try {
      await init()
      this.client = new MOQTClient(url)
      wtConnectStartedAtMs = performance.now()
      await this.client.start()
      console.info('[moqt][wt] connected', {
        url,
        elapsedMs: Math.round((performance.now() - wtConnectStartedAtMs) * 100) / 100
      })
      this.setupCallbacks()

      if (options.sendSetup === false) {
        this.serverSetupResolve = null
        return
      }

      const receiveServerSetup = new Promise<void>((resolve) => {
        this.serverSetupResolve = resolve
      })

      const versions = options.versions ?? new BigUint64Array([0xff00000en])
      const maxRequestId = options.maxRequestId ?? 100n
      await this.client.sendClientSetup(versions, maxRequestId)
      await receiveServerSetup
    } catch (error) {
      this.cleanupClient()
      if (wtConnectStartedAtMs !== null) {
        console.error('[moqt][wt] connect failed', {
          url,
          elapsedMs: Math.round((performance.now() - wtConnectStartedAtMs) * 100) / 100
        })
      }
      console.error('Failed to connect MoQT client:', error)
      throw error
    }
  }

  async sendClientSetup(versions: BigUint64Array, maxRequestId: bigint): Promise<void> {
    const client = this.requireConnectedClient()
    const receiveServerSetup = new Promise<void>((resolve) => {
      this.serverSetupResolve = resolve
    })
    await client.sendClientSetup(versions, maxRequestId)
    await receiveServerSetup
  }

  async disconnect(): Promise<void> {
    if (!this.client) {
      return
    }
    try {
      await this.client.close()
    } finally {
      this.cleanupClient()
    }
  }

  async finish(): Promise<void> {
    await this.disconnect()
  }

  getConnectionStatus(): this is MoqtClientWrapper & { client: MOQTClient } {
    return !!this.client && this.client.isConnected()
  }

  getRawClient(): MOQTClient | null {
    return this.client
  }

  setOnConnectionClosedHandler(handler: (() => void) | null): void {
    this.onConnectionClosedHandler = handler
  }

  setOnServerSetupHandler(handler: ((setup: ServerSetupMessage) => void) | null): void {
    this.onServerSetupHandler = handler
  }

  setOnPublishNamespaceHandler(handler: IncomingPublishNamespaceHandler | null): void {
    this.onPublishNamespaceHandler = handler
  }

  setOnPublishNamespaceDoneHandler(handler: IncomingPublishNamespaceDoneHandler | null): void {
    this.onPublishNamespaceDoneHandler = handler
  }

  setOnPublishNamespaceResponseHandler(handler: NamespaceResponseHandler): void {
    this.onPublishNamespaceResponseHandler = handler
  }

  setOnSubscribeNamespaceResponseHandler(handler: NamespaceResponseHandler): void {
    this.onSubscribeNamespaceResponseHandler = handler
  }

  setOnSubscribeResponseHandler(handler: SubscribeResponseHandler): void {
    this.onSubscribeResponseHandler = handler
  }

  setOnIncomingSubscribeHandler(handler: IncomingSubscribeHandler | null): void {
    this.incomingSubscribeHandler = handler
  }

  setOnIncomingUnsubscribeHandler(handler: IncomingUnsubscribeHandler | null): void {
    this.incomingUnsubscribeHandler = handler
  }

  setOnObjectDatagramHandler(handler: ObjectDatagramHandler): void {
    this.onObjectDatagramHandler = handler
  }

  setOnObjectDatagramStatusHandler(handler: ObjectDatagramStatusHandler): void {
    this.onObjectDatagramStatusHandler = handler
  }

  setOnSubgroupHeaderHandler(handler: SubgroupHeaderHandler): void {
    this.onSubgroupHeaderHandler = handler
  }

  setOnSubgroupObjectHandler(trackAlias: bigint, handler: SubgroupObjectHandler): void {
    this.subscriptionState.setSubgroupObjectHandler(trackAlias, handler)
  }

  clearSubgroupObjectHandler(trackAlias: bigint): void {
    this.subscriptionState.deleteSubgroupObjectHandler(trackAlias)
  }

  clearSubgroupObjectHandlers(): void {
    this.subscriptionState.clearHandlers()
  }

  async publishNamespace(
    trackNamespace: string[],
    authInfo: string,
    options: PublishNamespaceOptions = {}
  ): Promise<void> {
    const client = this.requireConnectedClient()
    const requestId = options.requestId ?? this.issueRequestId()
    const response = new Promise<void>((resolve, reject) => {
      this.pendingPublishNamespace.set(requestId, { resolve, reject })
    })
    await client.sendPublishNamespace(requestId, trackNamespace, authInfo)
    await response
  }

  async subscribeNamespace(
    trackNamespacePrefix: string[],
    authInfo: string,
    options: SubscribeNamespaceOptions = {}
  ): Promise<void> {
    const client = this.requireConnectedClient()
    const requestId = options.requestId ?? this.issueRequestId()
    const response = new Promise<void>((resolve, reject) => {
      this.pendingSubscribeNamespace.set(requestId, { resolve, reject })
    })
    await client.sendSubscribeNamespace(requestId, trackNamespacePrefix, authInfo)
    await response
  }

  async subscribe(
    trackNamespace: string[],
    trackName: string,
    authInfo: string,
    options: SubscribeOptions = {}
  ): Promise<SubscribeResult> {
    const client = this.requireConnectedClient()
    const requestId = options.requestId ?? this.issueRequestId()
    const existingTrackAlias = this.subscriptionTrackAliases.get(requestId)
    if (existingTrackAlias !== undefined) {
      // Only reachable when a caller passes an explicit options.requestId that
      // collides with an in-flight subscription; internally issued ids never do.
      throw new Error(`subscribe: requestId ${requestId} already has trackAlias ${existingTrackAlias}`)
    }

    const response = new Promise<SubscribeOkMessage>((resolve, reject) => {
      this.pendingSubscribe.set(requestId, { resolve, reject })
    })

    await client.sendSubscribe(
      requestId,
      trackNamespace,
      trackName,
      options.subscriberPriority ?? 0,
      options.groupOrder ?? 0,
      options.filterType ?? 1,
      options.startGroup,
      options.startObject,
      options.endGroup,
      authInfo,
      options.forward ?? false,
      options.deliveryTimeout
    )

    const subscribeOk = await response
    return { requestId, subscribeOk }
  }

  async fetch(
    trackNamespace: string[],
    trackName: string,
    startGroupId: bigint,
    startObjectId: bigint,
    endGroupId: bigint,
    endObjectId: bigint,
    options: FetchOptions = {}
  ): Promise<FetchResult> {
    const client = this.requireConnectedClient()
    const requestId = options.requestId ?? this.issueRequestId()
    if (options.onObject) {
      this.setOnFetchObjectHandler(requestId, options.onObject)
    }
    const response = this.awaitFetchOk()
    try {
      await client.sendFetch(requestId, trackNamespace, trackName, startGroupId, startObjectId, endGroupId, endObjectId)
      const fetchOk = await response
      return { requestId, fetchOk }
    } catch (error) {
      if (options.onObject) {
        this.clearFetchObjectHandler(requestId)
      }
      throw error
    }
  }

  async relativeJoiningFetch(
    joiningRequestId: bigint,
    joiningStart: bigint,
    options: FetchOptions = {}
  ): Promise<FetchResult> {
    const client = this.requireConnectedClient()
    const requestId = options.requestId ?? this.issueRequestId()
    if (options.onObject) {
      this.setOnFetchObjectHandler(requestId, options.onObject)
    }
    const response = this.awaitFetchOk()
    try {
      await client.sendRelativeJoiningFetch(requestId, joiningRequestId, joiningStart)
      const fetchOk = await response
      return { requestId, fetchOk }
    } catch (error) {
      if (options.onObject) {
        this.clearFetchObjectHandler(requestId)
      }
      throw error
    }
  }

  private awaitFetchOk(): Promise<FetchOkMessage> {
    return new Promise<FetchOkMessage>((resolve, reject) => {
      this.onFetchResponseHandler = (msg) => {
        this.onFetchResponseHandler = null
        if (msg instanceof FetchOkMessage) {
          resolve(msg)
        } else {
          reject(new Error(`FETCH_ERROR: ${(msg as RequestErrorMessage).reasonPhrase}`))
        }
      }
    })
  }

  setOnFetchObjectHandler(requestId: bigint, handler: FetchObjectHandler): void {
    this.fetchObjectHandlers.set(requestId, handler)
  }

  clearFetchObjectHandler(requestId: bigint): void {
    this.fetchObjectHandlers.delete(requestId)
  }

  async unsubscribe(requestId: bigint): Promise<void> {
    const client = this.requireConnectedClient()
    const trackAlias = this.subscriptionTrackAliases.get(requestId)
    await client.sendUnsubscribe(requestId)
    this.subscriptionTrackAliases.delete(requestId)
    if (trackAlias !== undefined) {
      this.clearSubgroupObjectHandler(trackAlias)
    }
  }

  async sendSubgroupTextForTrack(trackNamespace: string[], trackName: string, text: string): Promise<void> {
    const client = this.requireConnectedClient()
    const aliases = client.getTrackSubscribers(trackNamespace, trackName)
    if (!aliases.length) {
      console.warn(`No subscribers registered for track ${trackNamespace.join('/')}/${trackName}`)
      return
    }

    for (const alias of aliases) {
      await this.sendSubgroupTextForAlias(alias, text)
    }
  }

  private setupCallbacks(): void {
    if (!this.client) return

    this.client.onServerSetup((setup: ServerSetupMessage) => {
      if (this.serverSetupResolve) {
        this.serverSetupResolve()
        this.serverSetupResolve = null
      }
      this.onServerSetupHandler?.(setup)
    })

    this.client.onPublishNamespace((publishNamespace: PublishNamespaceMessage) => {
      const handler = this.onPublishNamespaceHandler ?? defaultIncomingPublishNamespaceHandler
      void handler({
        publishNamespace,
        respondOk: () => this.requireConnectedClient().sendPublishNamespaceOk(publishNamespace.requestId),
        respondError: (errorCode, reasonPhrase) =>
          this.requireConnectedClient().sendPublishNamespaceError(publishNamespace.requestId, errorCode, reasonPhrase)
      })
    })

    this.client.onPublishNamespaceDone((message: PublishNamespaceDoneMessage) => {
      this.onPublishNamespaceDoneHandler?.(message)
    })

    this.client.onPublishNamespaceResponse((response: NamespaceOkMessage | RequestErrorMessage) => {
      this.onPublishNamespaceResponseHandler?.(response)
      const pending = this.pendingPublishNamespace.get(response.requestId)
      if (!pending) {
        return
      }
      this.pendingPublishNamespace.delete(response.requestId)
      if (isRequestError(response)) {
        pending.reject(new Error(`PUBLISH_NAMESPACE_ERROR ${response.errorCode}: ${response.reasonPhrase}`))
      } else {
        pending.resolve()
      }
    })

    this.client.onSubscribeNamespaceResponse((response: NamespaceOkMessage | RequestErrorMessage) => {
      this.onSubscribeNamespaceResponseHandler?.(response)
      const pending = this.pendingSubscribeNamespace.get(response.requestId)
      if (!pending) {
        return
      }
      this.pendingSubscribeNamespace.delete(response.requestId)
      if (isRequestError(response)) {
        pending.reject(new Error(`SUBSCRIBE_NAMESPACE_ERROR ${response.errorCode}: ${response.reasonPhrase}`))
      } else {
        pending.resolve()
      }
    })

    this.client.onSubscribe(async (subscribe: SubscribeMessage, isSuccess: boolean, code: number) => {
      const handler = this.incomingSubscribeHandler ?? defaultIncomingSubscribeHandler
      await handler({
        subscribe,
        isSuccess,
        code,
        respondOk: async (
          expires = 0n,
          contentExists = false,
          largestGroupId,
          largestObjectId,
          deliveryTimeout,
          maxDuration
        ) =>
          this.requireConnectedClient().sendSubscribeOk(
            subscribe.requestId,
            expires,
            contentExists,
            largestGroupId,
            largestObjectId,
            deliveryTimeout,
            maxDuration
          ),
        respondError: (errorCode, reasonPhrase) =>
          this.requireConnectedClient().sendSubscribeError(subscribe.requestId, errorCode, reasonPhrase)
      })
    })

    this.client.onSubscribeResponse((response: SubscribeOkMessage | RequestErrorMessage) => {
      if (isRequestError(response)) {
        const pending = this.pendingSubscribe.get(response.requestId)
        if (pending) {
          this.pendingSubscribe.delete(response.requestId)
          pending.reject(new Error(`SUBSCRIBE_ERROR ${response.errorCode}: ${response.reasonPhrase}`))
        }
        this.subscriptionTrackAliases.delete(response.requestId)
      } else {
        this.subscriptionTrackAliases.set(response.requestId, response.trackAlias)
        const pending = this.pendingSubscribe.get(response.requestId)
        if (pending) {
          this.pendingSubscribe.delete(response.requestId)
          pending.resolve(response)
        }
      }
      this.onSubscribeResponseHandler?.(response)
    })

    this.client.onIncomingUnsubscribe((requestId: bigint) => {
      this.incomingUnsubscribeHandler?.(requestId)
    })

    this.client.onObjectDatagram((message: ObjectDatagramMessage) => {
      this.onObjectDatagramHandler?.(message)
    })
    this.client.onObjectDatagramStatus((message: ObjectDatagramStatusMessage) => {
      this.onObjectDatagramStatusHandler?.(message)
    })
    this.client.onSubgroupHeader((header: SubgroupHeaderMessage) => {
      this.onSubgroupHeaderHandler?.(header)
    })
    this.client.onSubgroupObject((trackAlias: bigint, groupId: bigint, subgroupObject: SubgroupObjectMessage) => {
      console.log('[moqt] received subgroup object', {
        trackAlias: trackAlias.toString(),
        groupId: groupId.toString(),
        subgroupId: subgroupObject.subgroupId?.toString() ?? '0',
        objectIdDelta: subgroupObject.objectIdDelta.toString(),
        objectPayloadLength: subgroupObject.objectPayloadLength,
        objectStatus: subgroupObject.objectStatus
      })
      const handler = this.subscriptionState.getSubgroupObjectHandler(trackAlias)
      if (handler) {
        handler(groupId, subgroupObject)
        return
      }
      this.subscriptionState.bufferSubgroupObject(trackAlias, groupId, subgroupObject)
    })
    this.client.onFetchResponse((response: FetchOkMessage | RequestErrorMessage) => {
      this.onFetchResponseHandler?.(response)
    })
    this.client.onFetchObject((message: FetchObjectMessage) => {
      const handler = this.fetchObjectHandlers.get(BigInt(message.requestId))
      handler?.(message)
    })
    this.client.onConnectionClosed(() => this.handleConnectionClosed())
  }

  private async sendSubgroupTextForAlias(trackAlias: bigint, text: string): Promise<void> {
    const client = this.requireConnectedClient()
    const state = client.getSubgroupState(trackAlias) as SubgroupState
    const publisherPriority = 0

    if (!state.headerSent) {
      await client.sendSubgroupHeader(trackAlias, state.groupId, state.subgroupId, publisherPriority)
      client.markSubgroupHeaderSent(trackAlias)
    }

    const payload = new TextEncoder().encode(text)
    await client.sendSubgroupObject(
      trackAlias,
      state.groupId,
      state.subgroupId,
      state.objectId,
      undefined,
      payload,
      undefined
    )
    client.incrementSubgroupObject(trackAlias)
  }

  // MOQ-T draft-14 §9.1: client-initiated Request IDs start at 0 and increase by
  // 2 (clients use even ids, servers odd). One counter covers every request type
  // (PUBLISH_NAMESPACE / SUBSCRIBE_NAMESPACE / SUBSCRIBE / FETCH) so ids stay
  // unique within the session.
  private issueRequestId(): bigint {
    const requestId = this.nextRequestId
    this.nextRequestId += 2n
    return requestId
  }

  private handleConnectionClosed(): void {
    this.cleanupClient()
    this.onConnectionClosedHandler?.()
  }

  private cleanupClient(): void {
    this.client = null
    this.serverSetupResolve = null
    this.pendingPublishNamespace.clear()
    this.pendingSubscribeNamespace.clear()
    this.pendingSubscribe.clear()
    this.subscriptionTrackAliases.clear()
    this.nextRequestId = 0n
    this.onPublishNamespaceHandler = null
    this.onPublishNamespaceDoneHandler = null
    this.onPublishNamespaceResponseHandler = null
    this.onSubscribeNamespaceResponseHandler = null
    this.onSubscribeResponseHandler = null
    this.onConnectionClosedHandler = null
    this.incomingSubscribeHandler = null
    this.incomingUnsubscribeHandler = null
    this.onServerSetupHandler = null
    this.onObjectDatagramHandler = null
    this.onObjectDatagramStatusHandler = null
    this.onSubgroupHeaderHandler = null
    this.clearSubgroupObjectHandlers()
  }

  private requireConnectedClient(): MOQTClient {
    if (!this.client || !this.client.isConnected()) {
      throw new Error('MOQT client is not connected')
    }
    return this.client
  }
}

function isRequestError(
  message: NamespaceOkMessage | RequestErrorMessage | SubscribeOkMessage
): message is RequestErrorMessage {
  return 'errorCode' in message
}

const defaultIncomingPublishNamespaceHandler: IncomingPublishNamespaceHandler = async ({ respondOk }) => {
  await respondOk()
}

const defaultIncomingSubscribeHandler: IncomingSubscribeHandler = async ({ code, respondError }) => {
  await respondError(BigInt(code || 500), 'subscribe rejected')
}
