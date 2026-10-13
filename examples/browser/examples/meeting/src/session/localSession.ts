import { PublishNamespaceDoneMessage, PublishNamespaceMessage, SubscribeOkMessage } from '../../../../pkg/moqt'
import { MoqtClientWrapper } from '@moqt/moqtClient'
import { CATALOG_TRACK_NAME } from '../../../../lib/msf/catalog'
import { LocalMember } from '../types/member'
import { ChatMessage } from '../types/chat'
import { MeetingMediaController } from '../media/meetingMediaController'
import { parseMeetingCatalogTracks } from '../media/meetingCatalog'
import type { MeetingCatalogTrack, CatalogSubscribeRole, TrackMediaConfig } from '../types/catalog'
import { readMeetingAuth } from './authToken'
import { buildNamespacePrefix, buildTrackNamespace, parseTrackNamespace } from './trackNamespace'

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
  private readonly mediaController: MeetingMediaController
  private state: LocalSessionState
  private chatMessageHandler: ((message: ChatMessage) => void) | null
  private readonly subscribeTrackAliases = new Map<bigint, bigint>()

  constructor({ roomName, userName, relayUrl, defaultAuthInfo = 'secret' }: LocalSessionOptions) {
    this.roomName = roomName
    this.relayUrl = relayUrl ?? 'https://relay-1.moqt.research.skyway.io:443'
    this.defaultAuthInfo = defaultAuthInfo
    this.localMember = { id: userName, name: userName }
    this.client.setOnConnectionClosedHandler(() => {
      console.info('[meeting][moqt] connection closed')
      this.transitionToState(LocalSessionState.Disconnected)
    })
    this.client.setOnServerSetupHandler((setup) => {
      console.info('[meeting][moqt] received SERVER_SETUP', setup)
    })
    this.client.setOnPublishNamespaceResponseHandler((response) => {
      console.info('[meeting][moqt] received PUBLISH_NAMESPACE response', response)
    })
    this.client.setOnSubscribeNamespaceResponseHandler((response) => {
      console.info('[meeting][moqt] received SUBSCRIBE_NAMESPACE response', response)
    })
    this.client.setOnSubscribeResponseHandler((response) => {
      if (response instanceof SubscribeOkMessage) {
        console.info('[meeting][moqt] received SUBSCRIBE_OK', {
          contentExists: (response as any).contentExists ?? (response as any).content_exists,
          largestGroupId: response.largestGroupId?.toString(),
          largestObjectId: response.largestObjectId?.toString()
        })
      } else {
        console.info('[meeting][moqt] received SUBSCRIBE response', response)
      }
    })
    this.mediaController = new MeetingMediaController(this.client, this.trackNamespace)
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
    return buildTrackNamespace(this.roomName, this.localMember.name)
  }

  get trackNamespacePrefix(): string[] {
    return buildNamespacePrefix(this.roomName)
  }

  setOnPublishNamespaceHandler(handler: ((publishNamespace: PublishNamespaceMessage) => void) | null): void {
    if (!handler) {
      this.client.setOnPublishNamespaceHandler(null)
      return
    }
    this.client.setOnPublishNamespaceHandler(async ({ publishNamespace, respondOk }) => {
      console.info('[meeting][moqt] received PUBLISH_NAMESPACE', publishNamespace)
      handler(publishNamespace)
      await respondOk()
    })
  }

  setOnPublishNamespaceDoneHandler(handler: ((message: PublishNamespaceDoneMessage) => void) | null): void {
    if (!handler) {
      this.client.setOnPublishNamespaceDoneHandler(null)
      return
    }
    this.client.setOnPublishNamespaceDoneHandler((message) => {
      console.info('[meeting][moqt] received PUBLISH_NAMESPACE_DONE', message.trackNamespace)
      handler(message)
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
      await this.client.connect(this.relayUrl, { authToken: readMeetingAuth().token })
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
      await this.unsubscribeAllActiveSubscriptions()
      await this.mediaController.dispose()
      this.subscribeTrackAliases.clear()
      this.chatMessageHandler = null
      this.client.setOnPublishNamespaceHandler(null)
      this.client.setOnSubscribeResponseHandler(null)
      this.client.setOnIncomingSubscribeHandler(null)
      this.client.setOnIncomingUnsubscribeHandler(null)
      this.client.setOnIncomingFetchHandler(null)
      this.client.setOnPublishNamespaceResponseHandler(null)
      this.client.setOnSubscribeNamespaceResponseHandler(null)
      this.client.setOnServerSetupHandler(null)
      this.client.setOnConnectionClosedHandler(null)
      await this.client.disconnect()
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
    trackNamespace: string[],
    trackName: string,
    authInfo: string = this.defaultAuthInfo,
    role?: CatalogSubscribeRole,
    config?: TrackMediaConfig
  ): Promise<bigint> {
    if (this.state !== LocalSessionState.Ready) {
      throw new Error(`Cannot subscribe when session state is "${this.state}"`)
    }
    const { requestId, subscribeOk } = await this.client.subscribe(trackNamespace, trackName, authInfo)
    const trackAlias = subscribeOk.trackAlias
    this.subscribeTrackAliases.set(requestId, trackAlias)
    const remoteUser = parseTrackNamespace(trackNamespace)?.userName ?? `alias-${trackAlias.toString()}`

    const resolvedRole = role ?? this.resolveTrackRole(trackName)
    if (resolvedRole === 'chat') {
      this.client.setOnSubgroupObjectHandler(trackAlias, (groupId, subgroup) => {
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
      const { subscriber } = this.mediaController
      if (resolvedRole === 'audio') {
        subscriber.registerAudioTrack(remoteUser, trackName, trackAlias, config)
      } else {
        subscriber.registerVideoTrack(remoteUser, trackName, trackAlias, config)
      }
    }

    return requestId
  }

  async subscribeCatalog(
    trackNamespace: string[],
    authInfo: string = this.defaultAuthInfo,
    timeoutMs: number = 5000,
    onTracksUpdated?: (tracks: MeetingCatalogTrack[]) => void
  ): Promise<MeetingCatalogTrack[]> {
    if (this.state !== LocalSessionState.Ready) {
      throw new Error(`Cannot subscribe catalog when session state is "${this.state}"`)
    }
    const {
      promise: firstTracks,
      resolve: resolveFirst,
      reject: rejectFirst
    } = Promise.withResolvers<MeetingCatalogTrack[]>()
    const subscribed = async () => {
      const { requestId, subscribeOk } = await this.client.subscribeLatest(
        trackNamespace,
        CATALOG_TRACK_NAME,
        authInfo,
        (payload, groupId) => {
          try {
            const tracks = parseMeetingCatalogTracks(new TextDecoder().decode(payload))
            console.info('[meeting][catalog] apply', {
              trackNamespace,
              groupId: groupId.toString(),
              tracks: tracks.map((t) => t.role)
            })
            onTracksUpdated?.(tracks)
            resolveFirst(tracks)
          } catch (error) {
            console.error('Failed to parse catalog payload:', error)
            rejectFirst(error instanceof Error ? error : new Error('Failed to parse catalog payload'))
          }
        }
      )
      this.subscribeTrackAliases.set(requestId, subscribeOk.trackAlias)
      return firstTracks
    }
    let timeoutId: number | undefined
    const timedOut = new Promise<never>((_, reject) => {
      timeoutId = window.setTimeout(() => reject(new Error('Catalog subscribe timed out')), timeoutMs)
    })
    try {
      return await Promise.race([subscribed(), timedOut])
    } finally {
      window.clearTimeout(timeoutId)
    }
  }

  async unsubscribe(subscribeId: bigint): Promise<void> {
    if (this.state !== LocalSessionState.Ready) {
      throw new Error(`Cannot unsubscribe when session state is "${this.state}"`)
    }

    const trackAlias = this.subscribeTrackAliases.get(subscribeId)
    this.subscribeTrackAliases.delete(subscribeId)
    if (trackAlias !== undefined) {
      this.mediaController.subscriber.unregisterTrack(trackAlias)
    }

    await this.client.unsubscribe(subscribeId)
  }

  private async unsubscribeAllActiveSubscriptions(): Promise<void> {
    const subscribeIds = Array.from(this.subscribeTrackAliases.keys())
    if (!subscribeIds.length) {
      return
    }
    await Promise.all(
      subscribeIds.map(async (subscribeId) => {
        try {
          await this.client.unsubscribe(subscribeId)
        } catch (error) {
          console.warn(`[meeting] failed to unsubscribe during disconnect (${subscribeId.toString()})`, error)
        }
      })
    )
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

  getMediaController(): MeetingMediaController {
    return this.mediaController
  }

  private resolveTrackRole(trackName: string): CatalogSubscribeRole | null {
    if (trackName === 'chat') {
      return 'chat'
    }
    const role = this.mediaController.publisher.resolveTrackRole(trackName)
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
