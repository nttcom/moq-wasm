import init, {
  FetchMessage,
  FetchObjectMessage,
  FetchOkMessage,
  FetchStreamEndMessage,
  MOQTClient,
  NamespaceOkMessage,
  ObjectDatagramMessage,
  ObjectDatagramStatusMessage,
  PublishDoneMessage,
  PublishNamespaceDoneMessage,
  PublishNamespaceMessage,
  PublishOkMessage,
  RequestErrorMessage,
  ServerSetupMessage,
  SubgroupHeaderMessage,
  SubgroupObjectMessage,
  SubgroupState,
  SubscribeMessage,
  SubscribeOkMessage
} from '../../pkg/moqt'
import { SubscriptionStateManager, type SubgroupObjectHandler } from './subscriptionStateManager'
import { SubgroupWriter } from './subgroupWriter'

type PendingResponse<T> = { resolve: (response: T) => void; reject: (error: Error) => void }
type PendingResponses<T> = Map<bigint, PendingResponse<T>>
type FetchObjectHandler = ((message: FetchObjectMessage) => void) | null
type FetchStreamEndHandler = (message: FetchStreamEndMessage) => void
type SubscribeResponseHandler = ((response: SubscribeOkMessage | RequestErrorMessage) => void) | null
type NamespaceResponseHandler = ((response: NamespaceOkMessage | RequestErrorMessage) => void) | null
export interface ConnectionCloseInfo {
  closeCode?: number
  reason?: string
}
type ConnectionClosedHandler = ((info: ConnectionCloseInfo) => void) | null
type IncomingUnsubscribeHandler = ((requestId: bigint) => void) | null
type PublishDoneHandler = (message: PublishDoneMessage) => void
type ObjectDatagramHandler = ((message: ObjectDatagramMessage) => void) | null
type ObjectDatagramStatusHandler = ((message: ObjectDatagramStatusMessage) => void) | null
type SubgroupHeaderHandler = ((header: SubgroupHeaderMessage) => void) | null

export interface ConnectOptions {
  sendSetup?: boolean
  versions?: BigUint64Array
  maxRequestId?: bigint
  authToken?: string
}

export interface PublishNamespaceOptions {
  requestId?: bigint
}

export interface SubscribeNamespaceOptions {
  requestId?: bigint
}

export const GroupOrder = { Publisher: 0, Ascending: 1, Descending: 2 } as const
export type GroupOrder = (typeof GroupOrder)[keyof typeof GroupOrder]

export const FilterType = { NextGroupStart: 0x1, LargestObject: 0x2, AbsoluteStart: 0x3, AbsoluteRange: 0x4 } as const
export type FilterType = (typeof FilterType)[keyof typeof FilterType]

export const RequestErrorCode = {
  InternalError: 0x0n,
  Timeout: 0x2n,
  NotSupported: 0x3n,
  TrackDoesNotExist: 0x4n,
  InvalidRange: 0x5n,
  NoObjects: 0x6n,
  UnknownStatusInRange: 0x8n
} as const

export class RequestError extends Error {
  readonly errorCode: bigint

  constructor(messageType: string, response: RequestErrorMessage) {
    super(`${messageType} ${response.errorCode}: ${response.reasonPhrase}`)
    this.errorCode = response.errorCode
  }
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
  groupOrder?: GroupOrder
  filterType?: FilterType
  startGroup?: bigint
  startObject?: bigint
  endGroup?: bigint
  forward?: boolean
  deliveryTimeout?: bigint
  /**
   * The subscription is over when this runs: its object handler is cleared
   * and objects still arriving on it are dropped, so there is nothing to
   * unsubscribe.
   */
  onPublishDone?: PublishDoneHandler
}

export interface FetchOptions {
  /** Override the request id; issued internally when omitted. */
  requestId?: bigint
  groupOrder?: GroupOrder
  /** Handler for FETCH objects, registered before the request is sent. */
  onObject?: FetchObjectHandler
  /**
   * Called once when the fetch stream ends: after the last object on a FIN,
   * or on a RESET_STREAM when the publisher could not complete the range.
   */
  onStreamEnd?: FetchStreamEndHandler
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
  respondOk(expires?: bigint, deliveryTimeout?: bigint, maxDuration?: bigint): Promise<bigint>
  respondError(code: bigint, reasonPhrase: string): Promise<void>
}

export interface IncomingTrackStatusContext {
  trackStatus: SubscribeMessage
  respondOk(): Promise<void>
  respondError(code: bigint, reasonPhrase: string): Promise<void>
}

export interface FetchObjectToSend {
  groupId: bigint
  subgroupId: bigint
  objectId: bigint
  publisherPriority: number
  objectStatus?: number
  payload: Uint8Array
  locHeader?: unknown
}

export interface IncomingFetchContext {
  fetch: FetchMessage
  cancelSignal: AbortSignal
  respondOk(): Promise<void>
  respondError(code: bigint, reasonPhrase: string): Promise<void>
  sendObject(object: FetchObjectToSend): Promise<void>
  finish(): Promise<void>
}

type IncomingPublishNamespaceHandler = (ctx: IncomingPublishNamespaceContext) => Promise<void> | void
type IncomingPublishNamespaceDoneHandler = (message: PublishNamespaceDoneMessage) => void
type IncomingSubscribeHandler = (ctx: IncomingSubscribeContext) => Promise<void> | void
type IncomingTrackStatusHandler = (ctx: IncomingTrackStatusContext) => Promise<void> | void
type IncomingFetchHandler = (ctx: IncomingFetchContext) => Promise<void> | void

export class MoqtClientWrapper {
  client: MOQTClient | null = null
  private pendingServerSetup: { resolve: () => void; reject: (error: Error) => void } | null = null
  private onPublishNamespaceHandler: IncomingPublishNamespaceHandler | null = null
  private onPublishNamespaceDoneHandler: IncomingPublishNamespaceDoneHandler | null = null
  private onPublishNamespaceResponseHandler: NamespaceResponseHandler = null
  private onSubscribeNamespaceResponseHandler: NamespaceResponseHandler = null
  private onSubscribeResponseHandler: SubscribeResponseHandler = null
  private onTrackStatusResponseHandler: SubscribeResponseHandler = null
  private readonly pendingTrackStatus: PendingResponses<SubscribeOkMessage> = new Map()
  private onConnectionClosedHandler: ConnectionClosedHandler = null
  private incomingSubscribeHandler: IncomingSubscribeHandler | null = null
  private incomingUnsubscribeHandler: IncomingUnsubscribeHandler = null
  private incomingTrackStatusHandler: IncomingTrackStatusHandler | null = null
  private incomingFetchHandler: IncomingFetchHandler | null = null
  private readonly incomingFetchCancellations = new Map<bigint, AbortController>()
  private onServerSetupHandler: ((setup: ServerSetupMessage) => void) | null = null
  private onObjectDatagramHandler: ObjectDatagramHandler = null
  private onObjectDatagramStatusHandler: ObjectDatagramStatusHandler = null
  private onSubgroupHeaderHandler: SubgroupHeaderHandler = null
  private readonly fetchObjectHandlers = new Map<bigint, FetchObjectHandler>()
  private readonly fetchStreamEndHandlers = new Map<bigint, FetchStreamEndHandler>()
  private readonly subscriptionState = new SubscriptionStateManager()
  private readonly pendingPublishNamespace: PendingResponses<NamespaceOkMessage> = new Map()
  private readonly pendingSubscribeNamespace: PendingResponses<NamespaceOkMessage> = new Map()
  private readonly pendingPublish: PendingResponses<PublishOkMessage> = new Map()
  private readonly pendingSubscribe: PendingResponses<SubscribeOkMessage> = new Map()
  private readonly pendingFetch: PendingResponses<FetchOkMessage> = new Map()
  private readonly subscriptionTrackAliases = new Map<bigint, bigint>()
  private readonly publishDoneHandlers = new Map<bigint, PublishDoneHandler>()
  private nextRequestId = 0n

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
        this.pendingServerSetup = null
        return
      }

      const receiveServerSetup = this.waitForServerSetup()

      const versions = options.versions ?? new BigUint64Array([0xff00000en])
      const maxRequestId = options.maxRequestId ?? 100n
      await this.client.sendClientSetup(versions, maxRequestId, options.authToken)
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

  async sendClientSetup(versions: BigUint64Array, maxRequestId: bigint, authToken?: string): Promise<void> {
    const client = this.requireConnectedClient()
    const receiveServerSetup = this.waitForServerSetup()
    await client.sendClientSetup(versions, maxRequestId, authToken)
    await receiveServerSetup
  }

  private waitForServerSetup(): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      this.pendingServerSetup = { resolve, reject }
    })
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

  getConnectionStatus(): this is MoqtClientWrapper & { client: MOQTClient } {
    return !!this.client && this.client.isConnected()
  }

  getRawClient(): MOQTClient | null {
    return this.client
  }

  setOnConnectionClosedHandler(handler: ((info: ConnectionCloseInfo) => void) | null): void {
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

  setOnTrackStatusResponseHandler(handler: SubscribeResponseHandler): void {
    this.onTrackStatusResponseHandler = handler
  }

  setOnIncomingSubscribeHandler(handler: IncomingSubscribeHandler | null): void {
    this.incomingSubscribeHandler = handler
  }

  setOnIncomingUnsubscribeHandler(handler: IncomingUnsubscribeHandler | null): void {
    this.incomingUnsubscribeHandler = handler
  }

  setOnIncomingTrackStatusHandler(handler: IncomingTrackStatusHandler | null): void {
    this.incomingTrackStatusHandler = handler
  }

  setOnIncomingFetchHandler(handler: IncomingFetchHandler | null): void {
    this.incomingFetchHandler = handler
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
    const response = expectResponse(this.pendingPublishNamespace, requestId)
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
    const response = expectResponse(this.pendingSubscribeNamespace, requestId)
    await client.sendSubscribeNamespace(requestId, trackNamespacePrefix, authInfo)
    await response
  }

  async publish(trackNamespace: string[], trackName: string, authInfo: string): Promise<bigint> {
    const client = this.requireConnectedClient()
    const requestId = this.issueRequestId()
    const response = expectResponse(this.pendingPublish, requestId)
    try {
      const trackAlias = await client.sendPublish(
        requestId,
        trackNamespace,
        trackName,
        undefined,
        GroupOrder.Ascending,
        false,
        undefined,
        undefined,
        true,
        authInfo
      )
      await response
      return trackAlias
    } catch (error) {
      this.pendingPublish.delete(requestId)
      throw error
    }
  }

  subscribe(
    trackNamespace: string[],
    trackName: string,
    authInfo: string,
    options: SubscribeOptions = {}
  ): Promise<SubscribeResult> {
    const requestId = options.requestId ?? this.issueRequestId()
    const result = this.sendSubscribe(requestId, trackNamespace, trackName, authInfo, options)
    const { onPublishDone } = options
    if (onPublishDone) {
      // PUBLISH_DONE can follow SUBSCRIBE_OK before the caller resumes from
      // the returned promise; chaining on that same promise runs the handler
      // after the caller's own continuation.
      this.publishDoneHandlers.set(requestId, (message) => void result.then(() => onPublishDone(message)))
    }
    return result
  }

  private async sendSubscribe(
    requestId: bigint,
    trackNamespace: string[],
    trackName: string,
    authInfo: string,
    options: SubscribeOptions
  ): Promise<SubscribeResult> {
    try {
      const client = this.requireConnectedClient()
      const existingTrackAlias = this.subscriptionTrackAliases.get(requestId)
      if (existingTrackAlias !== undefined) {
        // Only reachable when a caller passes an explicit options.requestId that
        // collides with an in-flight subscription; internally issued ids never do.
        throw new Error(`subscribe: requestId ${requestId} already has trackAlias ${existingTrackAlias}`)
      }

      const response = expectResponse(this.pendingSubscribe, requestId)
      await client.sendSubscribe(
        requestId,
        trackNamespace,
        trackName,
        options.subscriberPriority ?? 0,
        options.groupOrder ?? GroupOrder.Publisher,
        options.filterType ?? FilterType.NextGroupStart,
        options.startGroup,
        options.startObject,
        options.endGroup,
        authInfo,
        options.forward ?? false,
        options.deliveryTimeout
      )
      return { requestId, subscribeOk: await response }
    } catch (error) {
      this.publishDoneHandlers.delete(requestId)
      throw error
    }
  }

  fetch(
    trackNamespace: string[],
    trackName: string,
    startGroupId: bigint,
    startObjectId: bigint,
    endGroupId: bigint,
    endObjectId: bigint,
    options: FetchOptions = {}
  ): Promise<FetchResult> {
    return this.sendFetchRequest(options, (client, requestId, groupOrder) =>
      client.sendFetch(
        requestId,
        trackNamespace,
        trackName,
        startGroupId,
        startObjectId,
        endGroupId,
        endObjectId,
        groupOrder
      )
    )
  }

  relativeJoiningFetch(
    joiningRequestId: bigint,
    joiningStart: bigint,
    options: FetchOptions = {}
  ): Promise<FetchResult> {
    return this.sendFetchRequest(options, (client, requestId, groupOrder) =>
      client.sendRelativeJoiningFetch(requestId, joiningRequestId, joiningStart, groupOrder)
    )
  }

  /// Hands the objects and the stream end of a FETCH sent through the raw
  /// client to the handlers, as `fetch` does for its own. Call it before
  /// sending the FETCH: objects that arrive without a handler are dropped.
  watchFetch(requestId: bigint, onObject: FetchObjectHandler, onStreamEnd: FetchStreamEndHandler): void {
    this.fetchObjectHandlers.set(requestId, onObject)
    this.fetchStreamEndHandlers.set(requestId, onStreamEnd)
  }

  private async sendFetchRequest(
    options: FetchOptions,
    send: (client: MOQTClient, requestId: bigint, groupOrder: GroupOrder) => Promise<void>
  ): Promise<FetchResult> {
    const client = this.requireConnectedClient()
    const requestId = options.requestId ?? this.issueRequestId()
    if (options.onObject) {
      this.fetchObjectHandlers.set(requestId, options.onObject)
    }
    if (options.onStreamEnd) {
      this.fetchStreamEndHandlers.set(requestId, options.onStreamEnd)
    }
    const response = expectResponse(this.pendingFetch, requestId)
    try {
      await send(client, requestId, options.groupOrder ?? GroupOrder.Ascending)
      return { requestId, fetchOk: await response }
    } catch (error) {
      this.fetchObjectHandlers.delete(requestId)
      this.fetchStreamEndHandlers.delete(requestId)
      this.pendingFetch.delete(requestId)
      throw error
    }
  }

  async trackStatus(trackNamespace: string[], trackName: string, authInfo: string): Promise<SubscribeOkMessage> {
    const client = this.requireConnectedClient()
    const requestId = this.issueRequestId()
    const response = expectResponse(this.pendingTrackStatus, requestId)
    try {
      await client.sendTrackStatus(requestId, trackNamespace, trackName, authInfo)
      return await response
    } catch (error) {
      this.pendingTrackStatus.delete(requestId)
      throw error
    }
  }

  async unsubscribe(requestId: bigint): Promise<void> {
    const client = this.requireConnectedClient()
    const trackAlias = this.subscriptionTrackAliases.get(requestId)
    await client.sendUnsubscribe(requestId)
    this.subscriptionTrackAliases.delete(requestId)
    this.publishDoneHandlers.delete(requestId)
    if (trackAlias !== undefined) {
      this.clearSubgroupObjectHandler(trackAlias)
    }
  }

  async setSubscriptionForward(subscriptionRequestId: bigint, forward: boolean): Promise<void> {
    const client = this.requireConnectedClient()
    await client.sendSubscribeForward(this.issueRequestId(), subscriptionRequestId, forward)
  }

  /// A LargestObject SUBSCRIBE delivers only what follows the current largest
  /// object, so a joining FETCH supplies that one. The two race, so the newest
  /// group wins whichever arrives first.
  async subscribeLatest(
    trackNamespace: string[],
    trackName: string,
    authInfo: string,
    onLatest: (payload: Uint8Array, groupId: bigint) => void,
    options: Pick<SubscribeOptions, 'onPublishDone'> = {}
  ): Promise<SubscribeResult> {
    let newestGroupId: bigint | undefined
    const deliver = (groupId: bigint, payload: Uint8Array) => {
      if (payload.length === 0 || (newestGroupId !== undefined && groupId <= newestGroupId)) {
        return
      }
      newestGroupId = groupId
      onLatest(payload, groupId)
    }
    const result = await this.subscribe(trackNamespace, trackName, authInfo, {
      filterType: FilterType.LargestObject,
      forward: true,
      onPublishDone: options.onPublishDone
    })
    this.setOnSubgroupObjectHandler(result.subscribeOk.trackAlias, (groupId, object) => {
      if (object.objectStatus === undefined) {
        deliver(groupId, new Uint8Array(object.objectPayload))
      }
    })
    if (result.subscribeOk.contentExists) {
      await this.relativeJoiningFetch(result.requestId, 0n, {
        onObject: (object) => deliver(object.groupId, object.objectPayload)
      })
    }
    return result
  }

  async sendSingleObjectGroup(trackAlias: bigint, groupId: bigint, payload: Uint8Array): Promise<void> {
    const writer = new SubgroupWriter(this.requireConnectedClient(), trackAlias)
    await writer.openGroup(groupId)
    await writer.send(payload)
    await writer.closeGroup()
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
      if (this.pendingServerSetup) {
        this.pendingServerSetup.resolve()
        this.pendingServerSetup = null
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
      settle(this.pendingPublishNamespace, 'PUBLISH_NAMESPACE_ERROR', response)
    })

    this.client.onSubscribeNamespaceResponse((response: NamespaceOkMessage | RequestErrorMessage) => {
      this.onSubscribeNamespaceResponseHandler?.(response)
      settle(this.pendingSubscribeNamespace, 'SUBSCRIBE_NAMESPACE_ERROR', response)
    })

    this.client.onPublishResponse((response: PublishOkMessage | RequestErrorMessage) => {
      settle(this.pendingPublish, 'PUBLISH_ERROR', response)
    })

    this.client.onSubscribe(async (subscribe: SubscribeMessage, isSuccess: boolean, code: number) => {
      const handler = this.incomingSubscribeHandler ?? defaultIncomingSubscribeHandler
      await handler({
        subscribe,
        isSuccess,
        code,
        respondOk: async (expires = 0n, deliveryTimeout, maxDuration) =>
          this.requireConnectedClient().sendSubscribeOk(subscribe.requestId, expires, deliveryTimeout, maxDuration),
        respondError: (errorCode, reasonPhrase) =>
          this.requireConnectedClient().sendSubscribeError(subscribe.requestId, errorCode, reasonPhrase)
      })
    })

    this.client.onSubscribeResponse((response: SubscribeOkMessage | RequestErrorMessage) => {
      if (isRequestError(response)) {
        this.subscriptionTrackAliases.delete(response.requestId)
      } else {
        this.subscriptionTrackAliases.set(response.requestId, response.trackAlias)
      }
      settle(this.pendingSubscribe, 'SUBSCRIBE_ERROR', response)
      this.onSubscribeResponseHandler?.(response)
    })

    this.client.onIncomingUnsubscribe((requestId: bigint) => {
      this.incomingUnsubscribeHandler?.(requestId)
    })

    this.client.onPublishDone((message: PublishDoneMessage) => {
      const requestId = message.requestId
      console.info('[moqt] received PUBLISH_DONE', {
        requestId: requestId.toString(),
        statusCode: message.statusCode.toString(),
        errorReason: message.errorReason
      })
      const trackAlias = this.subscriptionTrackAliases.get(requestId)
      this.subscriptionTrackAliases.delete(requestId)
      if (trackAlias !== undefined) {
        this.subscriptionState.endSubscription(trackAlias)
      }
      const handler = this.publishDoneHandlers.get(requestId)
      this.publishDoneHandlers.delete(requestId)
      handler?.(message)
    })

    this.client.onObjectDatagram((message: ObjectDatagramMessage) => {
      this.onObjectDatagramHandler?.(message)
    })
    this.client.onObjectDatagramStatus((message: ObjectDatagramStatusMessage) => {
      this.onObjectDatagramStatusHandler?.(message)
    })
    this.client.onSubgroupHeader((header: SubgroupHeaderMessage) => {
      console.debug('[moqt] received subgroup header', {
        trackAlias: header.trackAlias.toString(),
        groupId: header.groupId.toString(),
        subgroupId: header.subgroupId.toString(),
        publisherPriority: header.publisherPriority
      })
      this.onSubgroupHeaderHandler?.(header)
    })
    this.client.onSubgroupObject((trackAlias: bigint, groupId: bigint, subgroupObject: SubgroupObjectMessage) => {
      const handler = this.subscriptionState.getSubgroupObjectHandler(trackAlias)
      if (handler) {
        handler(groupId, subgroupObject)
        return
      }
      this.subscriptionState.bufferSubgroupObject(trackAlias, groupId, subgroupObject)
    })
    this.client.onTrackStatus(async (trackStatus: SubscribeMessage) => {
      const handler = this.incomingTrackStatusHandler ?? defaultIncomingTrackStatusHandler
      await handler({
        trackStatus,
        respondOk: () => this.requireConnectedClient().sendTrackStatusOk(trackStatus.requestId),
        respondError: (errorCode, reasonPhrase) =>
          this.requireConnectedClient().sendTrackStatusError(trackStatus.requestId, errorCode, reasonPhrase)
      })
    })
    this.client.onTrackStatusResponse((response: SubscribeOkMessage | RequestErrorMessage) => {
      this.onTrackStatusResponseHandler?.(response)
      settle(this.pendingTrackStatus, 'TRACK_STATUS_ERROR', response)
    })
    this.client.onFetch(async (fetch: FetchMessage) => {
      const handler = this.incomingFetchHandler ?? defaultIncomingFetchHandler
      const requestId = fetch.requestId
      const cancellation = new AbortController()
      this.incomingFetchCancellations.set(requestId, cancellation)
      try {
        await handler({
          fetch,
          cancelSignal: cancellation.signal,
          respondOk: () => this.requireConnectedClient().sendFetchOk(requestId),
          respondError: (errorCode, reasonPhrase) =>
            this.requireConnectedClient().sendFetchError(requestId, errorCode, reasonPhrase),
          sendObject: (object) =>
            this.requireConnectedClient().sendFetchObject(
              requestId,
              object.groupId,
              object.subgroupId,
              object.objectId,
              object.publisherPriority,
              object.objectStatus,
              object.payload,
              object.locHeader
            ),
          finish: () => this.requireConnectedClient().finishFetch(requestId)
        })
      } finally {
        this.incomingFetchCancellations.delete(requestId)
      }
    })
    this.client.onFetchCancel((requestId: bigint) => {
      this.incomingFetchCancellations.get(requestId)?.abort()
    })
    this.client.onFetchResponse((response: FetchOkMessage | RequestErrorMessage) => {
      settle(this.pendingFetch, 'FETCH_ERROR', response)
    })
    this.client.onFetchObject((message: FetchObjectMessage) => {
      const handler = this.fetchObjectHandlers.get(BigInt(message.requestId))
      handler?.(message)
    })
    this.client.onFetchStreamEnd((message: FetchStreamEndMessage) => {
      const requestId = BigInt(message.requestId)
      const handler = this.fetchStreamEndHandlers.get(requestId)
      this.fetchStreamEndHandlers.delete(requestId)
      handler?.(message)
    })
    this.client.onConnectionClosed((info: ConnectionCloseInfo | null) => this.handleConnectionClosed(info ?? {}))
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
  // (PUBLISH_NAMESPACE / SUBSCRIBE_NAMESPACE / PUBLISH / SUBSCRIBE / FETCH) so ids stay
  // unique within the session.
  private issueRequestId(): bigint {
    const requestId = this.nextRequestId
    this.nextRequestId += 2n
    return requestId
  }

  private handleConnectionClosed(info: ConnectionCloseInfo): void {
    this.pendingServerSetup?.reject(new Error(describeConnectionClose(info)))
    this.cleanupClient()
    this.onConnectionClosedHandler?.(info)
  }

  private cleanupClient(): void {
    this.client = null
    this.pendingServerSetup = null
    this.pendingPublishNamespace.clear()
    this.pendingSubscribeNamespace.clear()
    this.pendingPublish.clear()
    this.pendingSubscribe.clear()
    this.pendingFetch.clear()
    this.fetchStreamEndHandlers.clear()
    this.subscriptionTrackAliases.clear()
    this.publishDoneHandlers.clear()
    this.nextRequestId = 0n
    this.onPublishNamespaceHandler = null
    this.onPublishNamespaceDoneHandler = null
    this.onPublishNamespaceResponseHandler = null
    this.onSubscribeNamespaceResponseHandler = null
    this.onSubscribeResponseHandler = null
    this.onConnectionClosedHandler = null
    this.incomingSubscribeHandler = null
    this.incomingUnsubscribeHandler = null
    this.incomingTrackStatusHandler = null
    this.incomingFetchHandler = null
    for (const cancellation of this.incomingFetchCancellations.values()) {
      cancellation.abort()
    }
    this.incomingFetchCancellations.clear()
    this.onServerSetupHandler = null
    this.onObjectDatagramHandler = null
    this.onObjectDatagramStatusHandler = null
    this.onSubgroupHeaderHandler = null
    this.clearSubgroupObjectHandlers()
  }

  requireConnectedClient(): MOQTClient {
    if (!this.client || !this.client.isConnected()) {
      throw new Error('MOQT client is not connected')
    }
    return this.client
  }
}

export function isRequestError<T extends object>(message: T | RequestErrorMessage): message is RequestErrorMessage {
  return 'errorCode' in message
}

function expectResponse<T>(pending: PendingResponses<T>, requestId: bigint): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    pending.set(requestId, { resolve, reject })
  })
}

function settle<T extends { requestId: bigint }>(
  pending: PendingResponses<T>,
  errorMessageType: string,
  response: T | RequestErrorMessage
): void {
  const resolver = pending.get(response.requestId)
  pending.delete(response.requestId)
  if (isRequestError(response)) {
    resolver?.reject(new RequestError(errorMessageType, response))
  } else {
    resolver?.resolve(response)
  }
}

const defaultIncomingPublishNamespaceHandler: IncomingPublishNamespaceHandler = async ({ respondOk }) => {
  await respondOk()
}

const defaultIncomingSubscribeHandler: IncomingSubscribeHandler = async ({ code, respondError }) => {
  await respondError(BigInt(code || 500), 'subscribe rejected')
}

const defaultIncomingTrackStatusHandler: IncomingTrackStatusHandler = async ({ respondError }) => {
  await respondError(RequestErrorCode.NotSupported, 'track status not supported')
}

const defaultIncomingFetchHandler: IncomingFetchHandler = async ({ respondError }) => {
  await respondError(RequestErrorCode.NotSupported, 'fetch not supported')
}

function describeConnectionClose(info: ConnectionCloseInfo): string {
  if (info.closeCode === undefined) {
    return 'connection closed before SERVER_SETUP was received'
  }
  const reason = info.reason ? `: ${info.reason}` : ''
  return `relay closed the connection before SERVER_SETUP (code ${info.closeCode}${reason})`
}
