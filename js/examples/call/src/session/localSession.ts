import { AnnounceMessage, SubscribeErrorMessage, SubscribeOkMessage } from '../../../../pkg/moqt_client_sample'
import { MoqtClientWrapper } from '@moqt/moqtClient'
import { LocalMember } from '../types/member'
import { ChatMessage } from '../types/chat'
import { CallMediaController } from '../media/callMediaController'

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
      this.transitionToState(LocalSessionState.Disconnected)
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

  setOnAnnounceHandler(handler: (announce: AnnounceMessage) => void): void {
    this.client.setOnAnnounceHandler(handler)
  }

  setOnSubscribeResponseHandler(handler: (response: SubscribeOkMessage | SubscribeErrorMessage) => void): void {
    this.client.setOnSubscribeResponseHandler(handler)
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
      await this.client.announce(this.trackNamespace, this.defaultAuthInfo)
      await this.client.subscribeAnnounces(this.trackNamespacePrefix, this.defaultAuthInfo)
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
      await this.client.disconnect()
    } finally {
      this.transitionToState(LocalSessionState.Disconnected)
    }
  }

  async announce(trackNamespace: string[], authInfo: string = this.defaultAuthInfo): Promise<void> {
    if (this.state !== LocalSessionState.Ready) {
      throw new Error(`Cannot announce tracks when session state is "${this.state}"`)
    }
    await this.client.announce(trackNamespace, authInfo)
  }

  async subscribeAnnounces(trackNamespacePrefix: string[], authInfo: string = this.defaultAuthInfo): Promise<void> {
    if (this.state !== LocalSessionState.Ready) {
      throw new Error(`Cannot subscribe announces when session state is "${this.state}"`)
    }
    await this.client.subscribeAnnounces(trackNamespacePrefix, authInfo)
  }

  async subscribe(
    subscribeId: bigint,
    trackAlias: bigint,
    trackNamespace: string[],
    trackName: string,
    authInfo: string = this.defaultAuthInfo
  ): Promise<void> {
    if (this.state !== LocalSessionState.Ready) {
      throw new Error(`Cannot subscribe when session state is "${this.state}"`)
    }
    const remoteUser = trackNamespace[1] ?? `alias-${trackAlias.toString()}`

    if (trackName === 'chat') {
      this.client.setOnSubgroupObjectHandler(trackAlias, (groupId, subgroup) => {
        try {
          const payload = new Uint8Array(subgroup.objectPayload)
          const text = new TextDecoder().decode(payload)
          if (this.chatMessageHandler) {
            const message: ChatMessage = {
              sender: remoteUser,
              trackNamespace: [...trackNamespace],
              groupId,
              text,
              timestamp: Date.now(),
              isLocal: false
            }
            this.chatMessageHandler(message)
          }
        } catch (error) {
          console.error('Failed to decode subgroup payload:', error)
        }
      })
    } else if (trackName === 'video' || trackName === 'audio') {
      this.mediaController.registerRemoteTrack(remoteUser, trackName, trackAlias)
    }

    await this.client.subscribe(subscribeId, trackAlias, trackNamespace, trackName, authInfo)
  }

  async sendChatMessage(message: string): Promise<void> {
    if (!message.trim()) {
      return
    }
    if (this.state !== LocalSessionState.Ready) {
      throw new Error(`Cannot send chat message when session state is "${this.state}"`)
    }
    await this.client.sendSubgroupTextForTrack(this.trackNamespace, 'chat', message)
  }

  getMediaController(): CallMediaController {
    return this.mediaController
  }
}
