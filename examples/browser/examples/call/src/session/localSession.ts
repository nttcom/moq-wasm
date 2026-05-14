import { PublishNamespaceMessage, RequestErrorMessage, SubscribeOkMessage } from '../../../../pkg/moqt_client_wasm'
import { MoqtClientWrapper } from '@moqt/moqtClient'
import { LocalMember } from '../types/member'
import { ChatMessage } from '../types/chat'
import { CallMediaController } from '../media/callMediaController'
import { parseCallCatalogTracks } from '../media/callCatalog'
import type { CallCatalogTrack, CatalogSubscribeRole } from '../types/catalog'

interface LocalSessionOptions {
  roomName: string
  userName: string
  relayUrl?: string
  defaultAuthInfo?: string
}

export enum LocalSessionState {
  Idle = 'idle',
  Connecting = 'connecting',
  Ready = 'ready',
  Disconnecting = 'disconnecting',
  Disconnected = 'disconnected'
}

export class LocalSession {
  readonly roomName: string
  private readonly relayUrl: string
  private readonly defaultAuthInfo: string
  readonly localMember: LocalMember
  private readonly client = new MoqtClientWrapper()
  private readonly mediaController: CallMediaController
  private state: LocalSessionState
  private chatMessageHandler: ((message: ChatMessage) => void) | null
  private readonly subscribeTrackAliases = new Map<bigint, bigint>()

  constructor({ roomName, userName, relayUrl, defaultAuthInfo = 'secret' }: LocalSessionOptions) {
    this.roomName = roomName
    this.relayUrl = relayUrl ?? 'https://moqt.research.skyway.io:4433'
    this.defaultAuthInfo = defaultAuthInfo
    this.localMember = {
      id: userName,
      name: userName,
      publishedTracks: {
        chat: true,
        video: false,
        audio: false
      }
    }
    this.client.setOnConnectionClosedHandler(() => {
      console.info('[call][moqt] connection closed')
      this.transitionToState(LocalSessionState.Disconnected)
    })
    this.client.setOnServerSetupHandler((setup) => {
      console.info('[call][moqt] received SERVER_SETUP', setup)
    })
    this.client.setOnPublishNamespaceResponseHandler((response) => {
      console.info('[call][moqt] received PUBLISH_NAMESPACE response', response)
    })
    this.client.setOnSubscribeNamespaceResponseHandler((response) => {
      console.info('[call][moqt] received SUBSCRIBE_NAMESPACE response', response)
    })
    this.mediaController = new CallMediaController(this.client, this.trackNamespace)
    this.state = LocalSessionState.Idle
    this.chatMessageHandler = null
  }

  get status(): LocalSessionState {
    return this.state
  }

  private transitionToState(next: LocalSessionState) {
    this.state = next
  }

  get trackNamespace(): string[] {
    return [this.roomName, this.localMember.name]
  }

  get trackNamespacePrefix(): string[] {
    return [this.roomName]
  }

  setOnPublishNamespaceHandler(handler: ((publishNamespace: PublishNamespaceMessage) => void) | null): void {
    if (!handler) {
      this.client.setOnPublishNamespaceHandler(null)
      return
    }
    this.client.setOnPublishNamespaceHandler(async ({ publishNamespace, respondOk }) => {
      console.info('[call][moqt] received PUBLISH_NAMESPACE', publishNamespace)
      handler(publishNamespace)
      await respondOk()
    })
  }

  setOnSubscribeResponseHandler(handler: (response: SubscribeOkMessage | RequestErrorMessage) => void): void {
    this.client.setOnSubscribeResponseHandler((response) => {
      console.info('[call][moqt] received SUBSCRIBE response', response)
      handler(response)
    })
  }

  setOnChatMessageHandler(handler: ((message: ChatMessage) => void) | null): void {
    this.chatMessageHandler = handler
  }

  async initialize(): Promise<void> {
    if (this.state === LocalSessionState.Ready || this.state === LocalSessionState.Connecting) {
      return
    }

    this.transitionToState(LocalSessionState.Connecting)
    try {
      await this.client.connect(this.relayUrl)
      this.transitionToState(LocalSessionState.Ready)
      await this.publishNamespace(this.trackNamespace, this.defaultAuthInfo)
    } catch (error) {
      this.transitionToState(LocalSessionState.Idle)
      throw error
    }
  }

  async disconnect(): Promise<void> {
    if (this.state === LocalSessionState.Idle || this.state === LocalSessionState.Disconnected) {
      return
    }

    this.transitionToState(LocalSessionState.Disconnecting)
    try {
      await this.mediaController.dispose()
      this.subscribeTrackAliases.clear()
      this.chatMessageHandler = null
      this.client.setOnPublishNamespaceHandler(null)
      this.client.setOnSubscribeResponseHandler(null)
      this.client.setOnIncomingSubscribeHandler(null)
      this.client.setOnIncomingUnsubscribeHandler(null)
      this.client.setOnPublishNamespaceResponseHandler(null)
      this.client.setOnSubscribeNamespaceResponseHandler(null)
      this.client.setOnServerSetupHandler(null)
      this.client.setOnConnectionClosedHandler(null)
      await this.client.finish()
    } finally {
      this.transitionToState(LocalSessionState.Disconnected)
    }
  }

  async publishNamespace(trackNamespace: string[], authInfo: string = this.defaultAuthInfo): Promise<void> {
    if (this.state !== LocalSessionState.Ready) {
      throw new Error(`Cannot publish namespace when session state is "${this.state}"`)
    }
    await this.client.publishNamespace(trackNamespace, authInfo)
  }

  async subscribeNamespace(trackNamespacePrefix: string[], authInfo: string = this.defaultAuthInfo): Promise<void> {
    if (this.state !== LocalSessionState.Ready) {
      throw new Error(`Cannot subscribe namespace when session state is "${this.state}"`)
    }
    await this.client.subscribeNamespace(trackNamespacePrefix, authInfo)
  }

  async subscribe(
    subscribeId: bigint,
    trackNamespace: string[],
    trackName: string,
    authInfo: string = this.defaultAuthInfo,
    role?: CatalogSubscribeRole,
    codec?: string
  ): Promise<bigint> {
    if (this.state !== LocalSessionState.Ready) {
      throw new Error(`Cannot subscribe when session state is "${this.state}"`)
    }
    const trackAlias = await this.client.subscribe(subscribeId, trackNamespace, trackName, authInfo)
    this.subscribeTrackAliases.set(subscribeId, trackAlias)
    const remoteUser = trackNamespace[1] ?? `alias-${trackAlias.toString()}`

    const resolvedRole = role ?? this.resolveTrackRole(trackName)
    if (resolvedRole === 'chat') {
      this.client.setOnSubgroupObjectHandler(trackAlias, (groupId, subgroup) => {
        console.info('[call][moqt] received subgroup object', {
          trackAlias: trackAlias.toString(),
          groupId: groupId.toString(),
          subgroupId: subgroup.subgroupId?.toString() ?? '0',
          objectIdDelta: subgroup.objectIdDelta.toString(),
          objectPayloadLength: subgroup.objectPayloadLength,
          objectStatus: subgroup.objectStatus
        })
        try {
          const payload = new Uint8Array(subgroup.objectPayload)
          const decoded = new TextDecoder().decode(payload)
          const { text, timestamp } = decodeChatPayload(decoded)
          if (this.chatMessageHandler) {
            const message: ChatMessage = {
              sender: remoteUser,
              trackNamespace: [...trackNamespace],
              groupId,
              text,
              timestamp,
              isLocal: false
            }
            this.chatMessageHandler(message)
          }
        } catch (error) {
          console.error('Failed to decode subgroup payload:', error)
        }
      })
    } else if (resolvedRole === 'video' || resolvedRole === 'screenshare' || resolvedRole === 'audio') {
      this.mediaController.registerRemoteTrack(remoteUser, trackName, trackAlias, resolvedRole, codec)
    }

    return trackAlias
  }

  async subscribeCatalog(
    subscribeId: bigint,
    trackNamespace: string[],
    authInfo: string = this.defaultAuthInfo,
    timeoutMs: number = 5000,
    onTracksUpdated?: (tracks: CallCatalogTrack[]) => void
  ): Promise<CallCatalogTrack[]> {
    if (this.state !== LocalSessionState.Ready) {
      throw new Error(`Cannot subscribe catalog when session state is "${this.state}"`)
    }
    return new Promise<CallCatalogTrack[]>((resolve, reject) => {
      let settled = false
      const timeoutId = window.setTimeout(() => {
        if (settled) {
          return
        }
        settled = true
        reject(new Error('Catalog subscribe timed out'))
      }, timeoutMs)

      this.client
        .subscribe(subscribeId, trackNamespace, 'catalog', authInfo)
        .then((trackAlias) => {
          this.subscribeTrackAliases.set(subscribeId, trackAlias)
          this.client.setOnSubgroupObjectHandler(trackAlias, (_groupId, subgroup) => {
            console.info('[call][moqt] received subgroup object', {
              trackAlias: trackAlias.toString(),
              groupId: _groupId.toString(),
              subgroupId: subgroup.subgroupId?.toString() ?? '0',
              objectIdDelta: subgroup.objectIdDelta.toString(),
              objectPayloadLength: subgroup.objectPayloadLength,
              objectStatus: subgroup.objectStatus
            })
            try {
              const payload = new TextDecoder().decode(new Uint8Array(subgroup.objectPayload))
              const tracks = parseCallCatalogTracks(payload)
              onTracksUpdated?.(tracks)
              if (settled) {
                return
              }
              settled = true
              window.clearTimeout(timeoutId)
              resolve(tracks)
            } catch (error) {
              if (settled) {
                console.error('Failed to parse updated catalog payload:', error)
                return
              }
              settled = true
              window.clearTimeout(timeoutId)
              reject(error instanceof Error ? error : new Error('Failed to parse catalog payload'))
            }
          })
        })
        .catch((error) => {
          if (settled) {
            return
          }
          settled = true
          window.clearTimeout(timeoutId)
          reject(error instanceof Error ? error : new Error('Catalog subscribe failed'))
        })
    })
  }

  async unsubscribe(subscribeId: bigint, role?: CatalogSubscribeRole): Promise<void> {
    if (this.state !== LocalSessionState.Ready) {
      throw new Error(`Cannot unsubscribe when session state is "${this.state}"`)
    }
    await this.client.unsubscribe(subscribeId)
    const trackAlias = this.subscribeTrackAliases.get(subscribeId)
    this.subscribeTrackAliases.delete(subscribeId)
    if (role === 'video' || role === 'screenshare' || role === 'audio') {
      if (trackAlias !== undefined) {
        this.mediaController.unregisterRemoteTrack(trackAlias, role)
      }
      return
    }
    if (trackAlias !== undefined) {
      this.mediaController.unregisterRemoteTrack(trackAlias)
    }
  }

  async sendChatMessage(message: string): Promise<void> {
    if (!message.trim()) {
      return
    }
    if (this.state !== LocalSessionState.Ready) {
      throw new Error(`Cannot send chat message when session state is "${this.state}"`)
    }
    await this.client.sendSubgroupTextForTrack(this.trackNamespace, 'chat', encodeChatPayload(message))
  }

  getMediaController(): CallMediaController {
    return this.mediaController
  }

  private resolveTrackRole(trackName: string): CatalogSubscribeRole | null {
    if (trackName === 'chat') {
      return 'chat'
    }
    const role = this.mediaController.resolveTrackRole(trackName)
    if (role === 'video' && trackName.trim().toLowerCase().startsWith('screenshare')) {
      return 'screenshare'
    }
    return role
  }
}

type ChatEventRecord = {
  t?: number
  data?: { text?: string }
}

function decodeChatPayload(payload: string): { text: string; timestamp: number } {
  const fallback = { text: payload, timestamp: Date.now() }
  try {
    const parsed = JSON.parse(payload)
    const record = Array.isArray(parsed) ? parsed[parsed.length - 1] : parsed
    if (!isObject(record)) {
      return fallback
    }
    const data = isObject(record.data) ? record.data : null
    const text = typeof data?.text === 'string' && data.text.trim() ? data.text : payload
    const timestamp = typeof record.t === 'number' && Number.isFinite(record.t) ? record.t : fallback.timestamp
    return { text, timestamp }
  } catch {
    return fallback
  }
}

function encodeChatPayload(message: string): string {
  const record: ChatEventRecord = { t: Date.now(), data: { text: message } }
  return JSON.stringify([record])
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}
