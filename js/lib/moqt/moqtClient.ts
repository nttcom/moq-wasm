import init, {
  MOQTClient,
  AnnounceMessage,
  ServerSetupMessage,
  AnnounceOkMessage,
  AnnounceErrorMessage,
  SubscribeMessage,
  SubscribeAnnouncesOkMessage,
  SubscribeAnnouncesErrorMessage,
  SubscribeOkMessage,
  SubscribeErrorMessage,
  SubgroupState,
  SubgroupStreamObjectMessage
} from '../../pkg/moqt_client_wasm'
import {
  InMemorySubscriptionStateManager,
  SubscriptionStateStore,
  SubgroupObjectHandler
} from './subscriptionStateManager'

type SetupResolver = ((value: void) => void) | null
type AnnounceHandler = ((announce: AnnounceMessage) => void) | null
type SubscribeResponseHandler = ((response: SubscribeOkMessage | SubscribeErrorMessage) => void) | null
type ConnectionClosedHandler = (() => void) | null

export interface ConnectOptions {
  sendSetup?: boolean
  versions?: BigUint64Array
  maxSubscribeId?: bigint
}

export interface IncomingSubscribeContext {
  subscribe: SubscribeMessage
  isSuccess: boolean
  code: number
  respondOk(expire: bigint, authInfo: string, forwardingPreference: string): Promise<void>
  respondError(code: bigint, reasonPhrase: string): Promise<void>
}

type IncomingSubscribeHandler = (ctx: IncomingSubscribeContext) => Promise<void> | void

export class MoqtClientWrapper {
  client: MOQTClient | null = null
  private serverSetupResolve: SetupResolver = null
  private onAnnounceHandler: AnnounceHandler = null
  private onSubscribeResponseHandler: SubscribeResponseHandler = null
  private onConnectionClosedHandler: ConnectionClosedHandler = null
  private incomingSubscribeHandler: IncomingSubscribeHandler | null = null
  private onServerSetupHandler: ((setup: ServerSetupMessage) => void) | null = null
  private readonly subscriptionState: SubscriptionStateStore

  constructor(subscriptionState?: SubscriptionStateStore) {
    this.subscriptionState = subscriptionState ?? new InMemorySubscriptionStateManager()
  }

  async connect(url: string, options: ConnectOptions = {}): Promise<void> {
    if (this.getConnectionStatus()) {
      return
    }

    try {
      await init()
      this.client = new MOQTClient(url)
      await this.client.start()
      this.setupCallbacks()

      if (options.sendSetup === false) {
        this.serverSetupResolve = null
        return
      }

      const receiveServerSetup = new Promise<void>((resolve) => {
        this.serverSetupResolve = resolve
      })

      const versions = options.versions ?? new BigUint64Array([0xff00000an])
      const maxSubscribeId = options.maxSubscribeId ?? 100n
      await this.client.sendSetupMessage(versions, maxSubscribeId)
      await receiveServerSetup
    } catch (error) {
      this.cleanupClient()
      console.error('Failed to connect MoQT client:', error)
      throw error
    }
  }

  async sendSetupMessage(versions: BigUint64Array, maxSubscribeId: bigint): Promise<void> {
    const client = this.requireConnectedClient()
    const receiveServerSetup = new Promise<void>((resolve) => {
      this.serverSetupResolve = resolve
    })
    await client.sendSetupMessage(versions, maxSubscribeId)
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

  getConnectionStatus(): this is MoqtClientWrapper & { client: MOQTClient } {
    return !!this.client && this.client.isConnected()
  }

  getRawClient(): MOQTClient | null {
    return this.client
  }

  setOnConnectionClosedHandler(handler: () => void): void {
    this.onConnectionClosedHandler = handler
  }

  setOnServerSetupHandler(handler: ((setup: ServerSetupMessage) => void) | null): void {
    this.onServerSetupHandler = handler
  }

  setOnAnnounceHandler(handler: (announce: AnnounceMessage) => void): void {
    this.onAnnounceHandler = handler
  }

  setOnSubscribeResponseHandler(handler: (response: SubscribeOkMessage | SubscribeErrorMessage) => void): void {
    this.onSubscribeResponseHandler = handler
  }

  setOnIncomingSubscribeHandler(handler: IncomingSubscribeHandler | null): void {
    this.incomingSubscribeHandler = handler
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

  async announce(trackNamespace: string[], authInfo: string): Promise<void> {
    const client = this.requireConnectedClient()
    await client.sendAnnounceMessage(trackNamespace, authInfo)
  }

  async subscribeAnnounces(trackNamespacePrefix: string[], authInfo: string): Promise<void> {
    const client = this.requireConnectedClient()
    await client.sendSubscribeAnnouncesMessage(trackNamespacePrefix, authInfo)
  }

  async subscribe(
    subscribeId: bigint,
    trackAlias: bigint,
    trackNamespace: string[],
    trackName: string,
    authInfo: string
  ): Promise<void> {
    const client = this.requireConnectedClient()
    if (client.isSubscribed(subscribeId)) {
      return
    }

    const priority = 0
    const groupOrder = 0
    const filterType = 1
    const startGroup = 0n
    const startObject = 0n
    const endGroup = 0n

    await client.sendSubscribeMessage(
      subscribeId,
      trackAlias,
      trackNamespace,
      trackName,
      priority,
      groupOrder,
      filterType,
      startGroup,
      startObject,
      endGroup,
      authInfo
    )
  }

  async unsubscribe(subscribeId: bigint): Promise<void> {
    const client = this.requireConnectedClient()
    await client.sendUnsubscribeMessage(subscribeId)
    this.clearSubgroupObjectHandler(subscribeId)
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

    this.client.onSetup((setup: ServerSetupMessage) => {
      if (this.serverSetupResolve) {
        this.serverSetupResolve()
        this.serverSetupResolve = null
      }
      this.onServerSetupHandler?.(setup)
    })

    this.client.onAnnounce((announce: AnnounceMessage) => {
      this.onAnnounceHandler?.(announce)
    })

    this.client.onAnnounceResponse((_response: AnnounceOkMessage | AnnounceErrorMessage) => {})

    this.client.onSubscribe(async (subscribe: SubscribeMessage, isSuccess: boolean, code: number) => {
      const handler = this.incomingSubscribeHandler ?? defaultIncomingSubscribeHandler
      await handler({
        subscribe,
        isSuccess,
        code,
        respondOk: (expire, authInfo, forwardingPreference) =>
          this.requireConnectedClient().sendSubscribeOkMessage(
            subscribe.subscribeId,
            expire,
            authInfo,
            forwardingPreference
          ),
        respondError: (errorCode, reasonPhrase) =>
          this.requireConnectedClient().sendSubscribeErrorMessage(subscribe.subscribeId, errorCode, reasonPhrase)
      })
    })

    this.client.onSubscribeResponse((response: SubscribeOkMessage | SubscribeErrorMessage) => {
      this.onSubscribeResponseHandler?.(response)
    })

    this.client.onSubscribeAnnouncesResponse(
      (_response: SubscribeAnnouncesOkMessage | SubscribeAnnouncesErrorMessage) => {}
    )
    this.client.onSubgroupStreamHeader((_header: any) => {})
    this.client.onSubgroupStreamObject(
      (trackAlias: bigint, groupId: bigint, subgroupObject: SubgroupStreamObjectMessage) => {
        const handler = this.subscriptionState.getSubgroupObjectHandler(trackAlias)
        handler?.(groupId, subgroupObject)
      }
    )
    this.client.onConnectionClosed(() => this.handleConnectionClosed())
  }

  private async sendSubgroupTextForAlias(trackAlias: bigint, text: string): Promise<void> {
    const client = this.requireConnectedClient()
    const state = client.getSubgroupState(trackAlias) as SubgroupState
    const publisherPriority = 0

    if (!state.headerSent) {
      await client.sendSubgroupStreamHeaderMessage(trackAlias, state.groupId, state.subgroupId, publisherPriority)
      client.markSubgroupHeaderSent(trackAlias)
    }

    const payload = new TextEncoder().encode(text)
    await client.sendSubgroupStreamObject(
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

  private handleConnectionClosed(): void {
    this.cleanupClient()
    this.onConnectionClosedHandler?.()
  }

  private cleanupClient(): void {
    if (this.client) {
      this.client.free()
      this.client = null
    }
    this.subscriptionState.clearHandlers()
    this.serverSetupResolve = null
  }

  private requireConnectedClient(): MOQTClient {
    if (!this.client || !this.client.isConnected()) {
      throw new Error('MOQT client is not connected')
    }
    return this.client
  }
}

const defaultIncomingSubscribeHandler: IncomingSubscribeHandler = async (ctx) => {
  if (ctx.isSuccess) {
    await ctx.respondOk(0n, 'secret', 'subgroup')
    return
  }
  await ctx.respondError(BigInt(ctx.code), 'Subscription validation failed')
}
